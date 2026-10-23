//! The chip behind the bus: its cores from the enumeration ROM (the AI
//! interconnect's agents), the ARM core held in reset or started, the RAM
//! the firmware goes into, and what the Raspberry Pi's chips are (ported
//! from bwfm.c's chip code).

use alloc::vec::Vec;

use crate::Error;
use crate::reg::*;

/// The backplane as the bus reaches it: 32-bit registers at their chip
/// addresses, and time.
pub trait Backplane {
    fn read32(&mut self, addr: u32) -> Result<u32, Error>;
    fn write32(&mut self, addr: u32, value: u32) -> Result<(), Error>;
    fn delay_us(&mut self, us: u32);
}

/// Chipcommon chip ids.
pub const CC_43430_CHIP_ID: u32 = 43430;
pub const CC_4345_CHIP_ID: u32 = 0x4345;

/// A chip the driver knows: the firmware files `wifi-firmware` keeps for it
/// (`FIRMWARE.bin`, `FIRMWARE.clm_blob`; the NVRAM `NVRAM.<board>.txt`).
pub struct Known {
    pub id: u32,
    pub name: &'static str,
    pub firmware: &'static str,
    pub nvram: &'static str,
    /// The SDIO CIS product id the card presents.
    pub product: u16,
}

/// Broadcom's SDIO manufacturer id.
pub const MANUFACTURER: u16 = 0x02d0;

pub const CHIPS: &[Known] = &[
    Known { id: CC_4345_CHIP_ID, name: "CYW43455", firmware: "cyfmac43455-sdio", nvram: "brcmfmac43455-sdio", product: 0xa9bf },
    Known { id: CC_43430_CHIP_ID, name: "CYW43438", firmware: "cyfmac43430-sdio", nvram: "brcmfmac43430-sdio", product: 0xa9a6 },
];

impl Known {
    pub fn by_product(product: u16) -> Option<&'static Known> {
        CHIPS.iter().find(|c| c.product == product)
    }
    pub fn by_id(id: u32) -> Option<&'static Known> {
        CHIPS.iter().find(|c| c.id == id)
    }
}

/// A core: its id and revision, its registers' base and its agent's.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Core {
    pub id: u16,
    pub rev: u8,
    pub base: u32,
    pub wrap: u32,
}

/// The chip as attached: its cores and what they say about it.
pub struct Chip {
    pub id: u32,
    pub rev: u32,
    pub cores: Vec<Core>,
    pub rambase: u32,
    pub ramsize: u32,
    pub srsize: u32,
    pub cc_caps: u32,
    pub cc_caps_ext: u32,
    pub pmucaps: u32,
    pub pmurev: u32,
}

impl Chip {
    /// No chip yet (before `attach`).
    pub const fn empty() -> Chip {
        Chip { id: 0, rev: 0, cores: Vec::new(), rambase: 0, ramsize: 0, srsize: 0, cc_caps: 0, cc_caps_ext: 0, pmucaps: 0, pmurev: 0 }
    }

    /// Read the chip's id and enumeration ROM, hold its CPU in reset (the
    /// firmware is not running yet) and size its RAM. The bus has prepared
    /// the backplane (its clock).
    pub fn attach(bp: &mut impl Backplane) -> Result<Chip, Error> {
        let val = bp.read32(CHIP_BASE + CHIP_REG_CHIPID)?;
        let mut chip = Chip {
            id: chipid_id(val),
            rev: chipid_rev(val),
            cores: Vec::new(),
            rambase: 0,
            ramsize: 0,
            srsize: 0,
            cc_caps: 0,
            cc_caps_ext: 0,
            pmucaps: 0,
            pmurev: 0,
        };
        match chipid_type(val) {
            CHIPID_TYPE_SOCI_AI => chip.erom_scan(bp)?,
            CHIPID_TYPE_SOCI_SB => return Err(Error::Chip("SB interconnect")),
            _ => return Err(Error::Chip("unknown interconnect")),
        }
        let has = |id| chip.core(id).is_some();
        if !(has(CORE_ARM_CM3) || has(CORE_ARM_CR4) || has(CORE_ARM_CA7)) {
            return Err(Error::Chip("no CPU core"));
        }
        if has(CORE_ARM_CM3) && !has(CORE_INTERNAL_MEM) {
            return Err(Error::Chip("no RAM core"));
        }
        chip.set_passive(bp)?;
        if let Some(core) = chip.core(CORE_ARM_CR4) {
            chip.ramsize = tcm_ramsize(bp, core)?;
            chip.rambase = tcm_rambase(chip.id, chip.rev)?;
        } else if let Some(core) = chip.core(CORE_SYS_MEM) {
            chip.ramsize = sysmem_ramsize(bp, &chip, core)?;
            chip.rambase = tcm_rambase(chip.id, chip.rev)?;
        } else if let Some(core) = chip.core(CORE_INTERNAL_MEM) {
            (chip.ramsize, chip.srsize) = socram_ramsize(bp, &chip, core)?;
        }
        let cc = chip.core(CORE_CHIPCOMMON).ok_or(Error::Chip("no chipcommon core"))?;
        chip.cc_caps = bp.read32(cc.base + CHIP_REG_CAPABILITIES)?;
        chip.cc_caps_ext = bp.read32(cc.base + CHIP_REG_CAPABILITIES_EXT)?;
        if chip.cc_caps & CHIP_REG_CAPABILITIES_PMU != 0 {
            let pmu = chip.pmu();
            chip.pmucaps = bp.read32(pmu.base + CHIP_REG_PMUCAPABILITIES)?;
            chip.pmurev = chip.pmucaps & CHIP_REG_PMUCAPABILITIES_REV_MASK;
        }
        Ok(chip)
    }

    pub fn core(&self, id: u16) -> Option<Core> {
        self.core_idx(id, 0)
    }

    pub fn core_idx(&self, id: u16, idx: usize) -> Option<Core> {
        self.cores.iter().filter(|c| c.id == id).nth(idx).copied()
    }

    /// The PMU's registers: its own core on newer chips, chipcommon's on
    /// the others.
    pub fn pmu(&self) -> Core {
        let cc = self.core(CORE_CHIPCOMMON).expect("a chipcommon core");
        if cc.rev >= 35 && self.cc_caps_ext & CHIP_REG_CAPABILITIES_EXT_AOB_PRESENT != 0 {
            if let Some(pmu) = self.core(CORE_PMU) {
                return pmu;
            }
        }
        cc
    }

    /// Walk the enumeration ROM: every component with a wrapper (and the
    /// PMU and GCI, which have none) is a core.
    fn erom_scan(&mut self, bp: &mut impl Backplane) -> Result<(), Error> {
        let mut erom = bp.read32(CHIP_BASE + CHIP_REG_EROMPTR)?;
        let mut kind = 0;
        while kind != DMP_DESC_EOT {
            let val = bp.read32(erom)?;
            kind = val & DMP_DESC_MASK;
            erom += 4;
            if kind != DMP_DESC_COMPONENT {
                continue;
            }
            let id = ((val & DMP_COMP_PARTNUM) >> DMP_COMP_PARTNUM_S) as u16;
            let val = bp.read32(erom)?;
            kind = val & DMP_DESC_MASK;
            erom += 4;
            if kind != DMP_DESC_COMPONENT {
                return Err(Error::Chip("EROM: not a component descriptor"));
            }
            let nmw = (val & DMP_COMP_NUM_MWRAP) >> DMP_COMP_NUM_MWRAP_S;
            let nsw = (val & DMP_COMP_NUM_SWRAP) >> DMP_COMP_NUM_SWRAP_S;
            let rev = ((val & DMP_COMP_REVISION) >> DMP_COMP_REVISION_S) as u8;
            if nmw + nsw == 0 && id != CORE_PMU && id != CORE_GCI {
                continue;
            }
            let Some((base, wrap)) = erom_regaddr(bp, &mut erom)? else {
                continue;
            };
            self.cores.insert(0, Core { id, rev, base, wrap });
        }
        Ok(())
    }

    /// Hold the CPU in reset, the 802.11 cores with it.
    pub fn set_passive(&self, bp: &mut impl Backplane) -> Result<(), Error> {
        if let Some(core) = self.core(CORE_ARM_CR4).or_else(|| self.core(CORE_ARM_CA7)) {
            let val = bp.read32(core.wrap + AGENT_IOCTL)?;
            ai_reset(bp, &core, val & AGENT_IOCTL_ARMCR4_CPUHALT, AGENT_IOCTL_ARMCR4_CPUHALT, AGENT_IOCTL_ARMCR4_CPUHALT)?;
            let mut i = 0;
            while let Some(d11) = self.core_idx(CORE_80211, i) {
                ai_disable(bp, &d11, AGENT_D11_IOCTL_PHYRESET | AGENT_D11_IOCTL_PHYCLOCKEN, AGENT_D11_IOCTL_PHYCLOCKEN)?;
                i += 1;
            }
            return Ok(());
        }
        if let Some(core) = self.core(CORE_ARM_CM3) {
            ai_disable(bp, &core, 0, 0)?;
            if let Some(d11) = self.core(CORE_80211) {
                ai_reset(bp, &d11, AGENT_D11_IOCTL_PHYRESET | AGENT_D11_IOCTL_PHYCLOCKEN, AGENT_D11_IOCTL_PHYCLOCKEN, AGENT_D11_IOCTL_PHYCLOCKEN)?;
            }
            let mem = self.core(CORE_INTERNAL_MEM).ok_or(Error::Chip("no RAM core"))?;
            ai_reset(bp, &mem, 0, 0, 0)?;
            if self.id == CC_43430_CHIP_ID {
                bp.write32(mem.base + SOCRAM_BANKIDX, 3)?;
                bp.write32(mem.base + SOCRAM_BANKPDA, 0)?;
            }
        }
        Ok(())
    }

    /// Start the CPU on the firmware in RAM: `activate` is the bus's part
    /// (the device core's interrupts cleared, the reset vector `rstvec`
    /// into RAM where the ARM CR4 and CA7 take it).
    pub fn set_active<B: Backplane>(&self, bp: &mut B, rstvec: u32, activate: impl FnOnce(&mut B, u32) -> Result<(), Error>) -> Result<(), Error> {
        if let Some(core) = self.core(CORE_ARM_CR4).or_else(|| self.core(CORE_ARM_CA7)) {
            activate(bp, rstvec)?;
            return ai_reset(bp, &core, AGENT_IOCTL_ARMCR4_CPUHALT, 0, 0);
        }
        if let Some(core) = self.core(CORE_ARM_CM3) {
            let mem = self.core(CORE_INTERNAL_MEM).ok_or(Error::Chip("no RAM core"))?;
            if !ai_isup(bp, &mem)? {
                return Err(Error::Chip("RAM core down"));
            }
            activate(bp, 0)?;
            return ai_reset(bp, &core, 0, 0, 0);
        }
        Err(Error::Chip("no CPU core"))
    }

    /// Whether the chip keeps its state through a sleep (save-restore),
    /// which lets the host leave the clock requests to it.
    pub fn sr_capable(&self, bp: &mut impl Backplane) -> Result<bool, Error> {
        if self.pmurev < 17 {
            return Ok(false);
        }
        match self.id {
            CC_4345_CHIP_ID => {
                let pmu = self.pmu();
                bp.write32(pmu.base + CHIP_REG_CHIPCONTROL_ADDR, 3)?;
                Ok(bp.read32(pmu.base + CHIP_REG_CHIPCONTROL_DATA)? & (1 << 2) != 0)
            }
            CC_43430_CHIP_ID => {
                let cc = self.core(CORE_CHIPCOMMON).ok_or(Error::Chip("no chipcommon core"))?;
                Ok(bp.read32(cc.base + CHIP_REG_SR_CONTROL1)? != 0)
            }
            _ => {
                let pmu = self.pmu();
                if bp.read32(pmu.base + CHIP_REG_PMUCAPABILITIES_EXT)? & CHIP_REG_PMUCAPABILITIES_SR_SUPP == 0 {
                    return Ok(false);
                }
                let ctl = bp.read32(pmu.base + CHIP_REG_RETENTION_CTL)?;
                Ok(ctl & (CHIP_REG_RETENTION_CTL_MACPHY_DIS | CHIP_REG_RETENTION_CTL_LOGIC_DIS) == 0)
            }
        }
    }
}

/// The register and wrapper addresses of the component at `erom`: `None`
/// when it has none the driver can use.
fn erom_regaddr(bp: &mut impl Backplane, erom: &mut u32) -> Result<Option<(u32, u32)>, Error> {
    let mut base = 0;
    let mut wrap = 0;
    let val = bp.read32(*erom)?;
    let kind = val & DMP_DESC_MASK;
    let wraptype = if kind == DMP_DESC_MASTER_PORT {
        *erom += 4;
        DMP_SLAVE_TYPE_MWRAP
    } else if kind & !DMP_DESC_ADDRSIZE_GT32 == DMP_DESC_ADDRESS {
        DMP_SLAVE_TYPE_SWRAP
    } else {
        return Ok(None);
    };
    loop {
        let mut val;
        loop {
            val = bp.read32(*erom)?;
            let kind = val & DMP_DESC_MASK;
            if kind == DMP_DESC_COMPONENT {
                return Ok(Some((base, wrap)));
            }
            if kind == DMP_DESC_EOT {
                return Ok(None);
            }
            *erom += 4;
            if kind & !DMP_DESC_ADDRSIZE_GT32 == DMP_DESC_ADDRESS {
                break;
            }
        }
        if val & DMP_DESC_ADDRSIZE_GT32 != 0 {
            *erom += 4;
        }
        let sztype = (val & DMP_SLAVE_SIZE_TYPE) >> DMP_SLAVE_SIZE_TYPE_S;
        if sztype == DMP_SLAVE_SIZE_DESC {
            let v = bp.read32(*erom)?;
            *erom += if v & DMP_DESC_ADDRSIZE_GT32 != 0 { 8 } else { 4 };
        }
        if sztype != DMP_SLAVE_SIZE_4K && sztype != DMP_SLAVE_SIZE_8K {
            continue;
        }
        let stype = (val & DMP_SLAVE_TYPE) >> DMP_SLAVE_TYPE_S;
        if base == 0 && stype == DMP_SLAVE_TYPE_SLAVE {
            base = val & DMP_SLAVE_ADDR_BASE;
        }
        if wrap == 0 && stype == wraptype {
            wrap = val & DMP_SLAVE_ADDR_BASE;
        }
        if base != 0 && wrap != 0 {
            return Ok(Some((base, wrap)));
        }
    }
}

/// The core is clocked and out of reset.
pub fn ai_isup(bp: &mut impl Backplane, core: &Core) -> Result<bool, Error> {
    let ioctl = bp.read32(core.wrap + AGENT_IOCTL)?;
    let reset = bp.read32(core.wrap + AGENT_RESET_CTL)?;
    Ok(ioctl & (AGENT_IOCTL_FGC | AGENT_IOCTL_CLK) == AGENT_IOCTL_CLK && reset & AGENT_RESET_CTL_RESET == 0)
}

/// Put the core in reset, with `prereset` in its ioctl before and `reset`
/// while it is.
pub fn ai_disable(bp: &mut impl Backplane, core: &Core, prereset: u32, reset: u32) -> Result<(), Error> {
    if bp.read32(core.wrap + AGENT_RESET_CTL)? & AGENT_RESET_CTL_RESET == 0 {
        bp.write32(core.wrap + AGENT_IOCTL, prereset | AGENT_IOCTL_FGC | AGENT_IOCTL_CLK)?;
        bp.read32(core.wrap + AGENT_IOCTL)?;
        bp.write32(core.wrap + AGENT_RESET_CTL, AGENT_RESET_CTL_RESET)?;
        bp.delay_us(20);
        let mut i = 300;
        while i > 0 && bp.read32(core.wrap + AGENT_RESET_CTL)? != AGENT_RESET_CTL_RESET {
            i -= 1;
        }
        if i == 0 {
            return Err(Error::Chip("core reset timeout"));
        }
    }
    bp.write32(core.wrap + AGENT_IOCTL, reset | AGENT_IOCTL_FGC | AGENT_IOCTL_CLK)?;
    bp.read32(core.wrap + AGENT_IOCTL)?;
    Ok(())
}

/// Reset the core and bring it up with `postreset` in its ioctl.
pub fn ai_reset(bp: &mut impl Backplane, core: &Core, prereset: u32, reset: u32, postreset: u32) -> Result<(), Error> {
    ai_disable(bp, core, prereset, reset)?;
    let mut i = 50;
    while i > 0 && bp.read32(core.wrap + AGENT_RESET_CTL)? & AGENT_RESET_CTL_RESET != 0 {
        bp.write32(core.wrap + AGENT_RESET_CTL, 0)?;
        bp.delay_us(60);
        i -= 1;
    }
    if i == 0 {
        return Err(Error::Chip("core reset timeout"));
    }
    bp.write32(core.wrap + AGENT_IOCTL, postreset | AGENT_IOCTL_CLK)?;
    bp.read32(core.wrap + AGENT_IOCTL)?;
    Ok(())
}

/// The SOCRAM core's banks: the RAM, and how much of it is retained
/// through a sleep.
fn socram_ramsize(bp: &mut impl Backplane, chip: &Chip, core: Core) -> Result<(u32, u32), Error> {
    if !ai_isup(bp, &core)? {
        ai_reset(bp, &core, 0, 0, 0)?;
    }
    let coreinfo = bp.read32(core.base + SOCRAM_COREINFO)?;
    let mut nb = (coreinfo & SOCRAM_COREINFO_SRNB_MASK) >> SOCRAM_COREINFO_SRNB_SHIFT;
    let (mut ramsize, mut srsize) = (0u32, 0u32);
    if core.rev <= 7 || core.rev == 12 {
        let banksize = coreinfo & SOCRAM_COREINFO_SRBSZ_MASK;
        let lss = (coreinfo & SOCRAM_COREINFO_LSS_MASK) >> SOCRAM_COREINFO_LSS_SHIFT;
        if lss != 0 {
            nb -= 1;
        }
        ramsize = nb * (1 << (banksize + SOCRAM_COREINFO_SRBSZ_BASE));
        if lss != 0 {
            ramsize += 1 << ((lss - 1) + SOCRAM_COREINFO_SRBSZ_BASE);
        }
    } else {
        for i in 0..nb {
            bp.write32(core.base + SOCRAM_BANKIDX, (SOCRAM_BANKIDX_MEMTYPE_RAM << SOCRAM_BANKIDX_MEMTYPE_SHIFT) | i)?;
            let bankinfo = bp.read32(core.base + SOCRAM_BANKINFO)?;
            let banksize = ((bankinfo & SOCRAM_BANKINFO_SZMASK) + 1) * SOCRAM_BANKINFO_SZBASE;
            ramsize += banksize;
            if bankinfo & SOCRAM_BANKINFO_RETNTRAM_MASK != 0 {
                srsize += banksize;
            }
        }
    }
    if chip.id == CC_43430_CHIP_ID {
        srsize = 64 * 1024;
    }
    Ok((ramsize, srsize))
}

fn sysmem_ramsize(bp: &mut impl Backplane, _chip: &Chip, core: Core) -> Result<u32, Error> {
    if !ai_isup(bp, &core)? {
        ai_reset(bp, &core, 0, 0, 0)?;
    }
    let coreinfo = bp.read32(core.base + SOCRAM_COREINFO)?;
    let nb = (coreinfo & SOCRAM_COREINFO_SRNB_MASK) >> SOCRAM_COREINFO_SRNB_SHIFT;
    let mut ramsize = 0;
    for i in 0..nb {
        bp.write32(core.base + SOCRAM_BANKIDX, (SOCRAM_BANKIDX_MEMTYPE_RAM << SOCRAM_BANKIDX_MEMTYPE_SHIFT) | i)?;
        let bankinfo = bp.read32(core.base + SOCRAM_BANKINFO)?;
        ramsize += ((bankinfo & SOCRAM_BANKINFO_SZMASK) + 1) * SOCRAM_BANKINFO_SZBASE;
    }
    Ok(ramsize)
}

/// The ARM CR4's tightly coupled memory, bank by bank.
fn tcm_ramsize(bp: &mut impl Backplane, core: Core) -> Result<u32, Error> {
    let cap = bp.read32(core.base + ARMCR4_CAP)?;
    let nab = cap & ARMCR4_CAP_TCBANB_MASK;
    let nbb = (cap & ARMCR4_CAP_TCBBNB_MASK) >> ARMCR4_CAP_TCBBNB_SHIFT;
    let mut ramsize = 0;
    for i in 0..nab + nbb {
        bp.write32(core.base + ARMCR4_BANKIDX, i)?;
        let bxinfo = bp.read32(core.base + ARMCR4_BANKINFO)?;
        let blksize = if bxinfo & ARMCR4_BANKINFO_BLK_1K_MASK != 0 { 1024 } else { 8192 };
        ramsize += ((bxinfo & ARMCR4_BANKINFO_BSZ_MASK) + 1) * blksize;
    }
    Ok(ramsize)
}

/// Where a CR4's or CA7's RAM starts, per chip.
fn tcm_rambase(id: u32, rev: u32) -> Result<u32, Error> {
    Ok(match id {
        CC_4345_CHIP_ID => 0x19_8000,
        0x4335 | 0x4339 | 0x4350 | 0x4354 | 0x4356 | 43567 | 43569 | 43570 | 0x4358 | 43602 | 0x4371 => 0x18_0000,
        43465 | 43525 | 0x4365 | 0x4366 | 43664 | 43666 => 0x20_0000,
        0x4359 => {
            if rev < 9 {
                0x18_0000
            } else {
                0x16_0000
            }
        }
        0x4355 | 0x4364 | 0x4373 => 0x16_0000,
        0x4377 | 43752 => 0x17_0000,
        0x4378 => 0x35_2000,
        0x4387 => 0x74_0000,
        _ => return Err(Error::Chip("unknown chip: no RAM base")),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_chips() {
        assert_eq!(Known::by_product(0xa9bf).unwrap().id, CC_4345_CHIP_ID);
        assert_eq!(Known::by_product(0xa9a6).unwrap().firmware, "cyfmac43430-sdio");
        assert!(Known::by_product(0x1234).is_none());
        assert_eq!(tcm_rambase(CC_4345_CHIP_ID, 6).unwrap(), 0x19_8000);
        assert!(tcm_rambase(1, 0).is_err());
    }
}
