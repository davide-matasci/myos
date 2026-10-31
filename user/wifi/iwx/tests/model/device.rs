//! The device side of the model: its registers (BAR0) and periphery, its
//! interrupt (the MSI causes with the ICT, or MSI-X with its automask),
//! the context information the firmware's self load reads, the receive
//! ring the firmware fills and the transmit queues it reads, all in the
//! memory the driver gave it (`Mem`). What the firmware does with the
//! commands is `firmware.rs`.
//!
//! It is strict: what a real device would not take (a register it does not
//! have, the periphery reached without the NIC awake, a descriptor that
//! does not add up, memory the device was not given) is a violation. The
//! first one is kept (`failed`), printed, and stops the firmware with an
//! error the driver reads in its log (SW_ERR).

use std::collections::{HashMap, VecDeque};
use std::time::{Duration, Instant};

use iwx::reg::*;

use super::firmware::Fw;
use super::air::Air;

/// How the host takes the device's interrupt.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IrqMode {
    Msi,
    Msix,
}

/// Where the device's interrupt goes: `set` follows its level (an MSI or
/// MSI-X message on each rise).
pub trait Irq: Send {
    fn set(&mut self, level: bool);
    fn mode(&self) -> IrqMode;
}

/// Memory the device was given: {bus address, length, host pointer,
/// whether the device may write it}.
struct Mapping {
    iova: u64,
    len: u64,
    ptr: *mut u8,
    writable: bool,
}

// SAFETY: the mappings are the driver's leaked DMA buffers, used under the
// model's lock only.
unsafe impl Send for Mapping {}

#[derive(Default)]
pub struct Mem {
    maps: Vec<Mapping>,
}

impl Mem {
    /// # Safety
    /// `ptr` is `len` bytes the device may read (and write, `writable`)
    /// until unmapped.
    pub unsafe fn map(&mut self, iova: u64, len: u64, ptr: *mut u8, writable: bool) {
        self.maps.push(Mapping { iova, len, ptr, writable });
    }

    /// Each piece of [addr, addr + len): its host pointer and length.
    fn pieces(&self, addr: u64, len: usize, write: bool) -> Option<Vec<(*mut u8, usize)>> {
        let mut out = Vec::new();
        let (mut at, mut left) = (addr, len as u64);
        while left > 0 {
            let m = self.maps.iter().find(|m| at >= m.iova && at < m.iova + m.len && (m.writable || !write))?;
            let n = left.min(m.iova + m.len - at);
            // SAFETY: inside the mapping (above).
            out.push((unsafe { m.ptr.add((at - m.iova) as usize) }, n as usize));
            at += n;
            left -= n;
        }
        Some(out)
    }

    pub fn read(&self, addr: u64, out: &mut [u8]) -> bool {
        let Some(pieces) = self.pieces(addr, out.len(), false) else {
            return false;
        };
        let mut off = 0;
        for (p, n) in pieces {
            // SAFETY: the mapping's memory (Mem::map), not `out`.
            unsafe { std::ptr::copy_nonoverlapping(p, out[off..].as_mut_ptr(), n) };
            off += n;
        }
        true
    }

    pub fn write(&self, addr: u64, data: &[u8]) -> bool {
        let Some(pieces) = self.pieces(addr, data.len(), true) else {
            return false;
        };
        let mut off = 0;
        for (p, n) in pieces {
            // SAFETY: as in read.
            unsafe { std::ptr::copy_nonoverlapping(data[off..].as_ptr(), p, n) };
            off += n;
        }
        true
    }
}

/// The radio modelled: what differs between the parts the driver takes.
pub struct Hardware {
    pub name: &'static str,
    /// HW_REV as the register reads.
    pub hw_rev_raw: u32,
    /// Integrated (CNVi): its latency tolerance in the periphery
    /// (HPM_UMAC_LTR), a SoC configuration without DISCRETE; else a PCIe
    /// card's: LTR_LONG_VAL_AD, DISCRETE.
    pub integrated: bool,
    /// The crystal latency the firmware must be told.
    pub xtal_latency_us: u32,
}

/// The AX201: a QuZ MAC, step B (0x354 in the driver's form), integrated.
pub const AX201: Hardware = Hardware { name: "AX201", hw_rev_raw: 0x351, integrated: true, xtal_latency_us: 500 };
/// The AX200: a CC MAC, step A, discrete.
pub const AX200: Hardware = Hardware { name: "AX200", hw_rev_raw: 0x340, integrated: false, xtal_latency_us: 0 };
/// Its MAC address (locally administered), in the OTP words.
pub const MAC: [u8; 6] = [0x02, 0x00, 0x5e, 0x00, 0x02, 0x01];
/// The MSI-X vectors (the driver routes every cause to the first).
pub const MSIX_VECTORS: u32 = 16;

// The rings as the context information sets them up.
pub const RX_RING: usize = 512;
pub const RX_BUF: usize = 4096;
pub const CMD_RING: usize = 256;
pub const TFD_LEN: usize = 256;
const FRAME_INVALID: u32 = 0x5555_0000;
/// The receive buffers waiting for a free buffer at most; later frames are
/// dropped (a command's answer never is).
const BACKLOG: usize = 1024;
/// The RX periodic interrupt's one shot after a real one.
const PERIODIC: Duration = Duration::from_millis(8);

/// The device's interrupt causes, as the firmware raises them: mapped to
/// CSR_INT (MSI, INTx) or the MSI-X cause registers.
#[derive(Clone, Copy)]
pub enum Cause {
    Alive,
    Rx,
    SwErr,
}

pub struct Device {
    pub hw: &'static Hardware,
    pub mem: Mem,
    irq: Box<dyn Irq>,
    start: Instant,
    level: bool,
    pub failed: Option<String>,

    hw_if_config: u32,
    gp_cntrl: u32,
    gio: u32,
    gio_chicken: u32,
    hpet: u32,
    link_pwr: u32,
    mbox: u32,
    shadow: u32,
    ucode_gp1: u32,
    /// LTR_LONG_VAL_AD.
    ltr: u32,
    int_coalescing: u8,
    periodic: Option<Instant>,
    ctxt_ba: u64,
    ict: Option<u64>,
    ict_cur: usize,
    int: u32,
    int_mask: u32,
    fh_int_status: u32,
    msix_fh: u32,
    msix_hw: u32,
    msix_fh_mask: u32,
    msix_hw_mask: u32,
    automasked: bool,
    ivar: [u8; 0x50],
    prph: HashMap<u32, u32>,
    prph_waddr: u32,
    prph_raddr: u32,
    mem_raddr: u32,
    /// The device's memory the driver reads through the HBUS window: the
    /// firmware's error log.
    sram: HashMap<u32, u32>,

    /// The receive ring once the firmware set it up: where its next
    /// buffer goes, the closed count, the driver's free index, and what
    /// waits for a free buffer.
    pub rx: Option<RxRing>,
    backlog: VecDeque<Vec<u8>>,
    pub fw: Option<Fw>,
    /// The driver stopped the device's DMA (RESET's STOP_MASTER), which
    /// the device acknowledges with MASTER_DISABLED.
    pub master_stopped: bool,
    /// The access points the radio reaches: they keep their stations
    /// across the device's resets.
    pub air: Air,
}

/// The receive ring of the context information.
pub struct RxRing {
    pub free_rbd: u64,
    pub used_rbd: u64,
    pub status: u64,
    pos: usize,
    closed: u16,
    widx: usize,
}

impl RxRing {
    pub fn new(free_rbd: u64, used_rbd: u64, status: u64) -> Self {
        RxRing { free_rbd, used_rbd, status, pos: 0, closed: 0, widx: 0 }
    }

    fn free(&self) -> usize {
        (self.widx + RX_RING - self.pos) % RX_RING
    }
}

/// A transmit queue's descriptor ring: where its descriptors and byte
/// counts are, the next one the device reads.
pub struct Ring {
    pub tfds: u64,
    pub byte_counts: Option<u64>,
    pub read: usize,
}

/// A descriptor's transfer buffers {bus address, length}.
pub fn tbs(tfd: &[u8]) -> Vec<(u64, usize)> {
    let n = (le16(tfd, 0) & 0x1f) as usize;
    (0..n.min(25)).map(|i| (le64(tfd, 4 + 10 * i), le16(tfd, 2 + 10 * i) as usize)).collect()
}

impl Device {
    pub fn new(hw: &'static Hardware, irq: Box<dyn Irq>) -> Self {
        let mut d = Device {
            hw,
            mem: Mem::default(),
            irq,
            start: Instant::now(),
            level: false,
            failed: None,
            hw_if_config: 0,
            gp_cntrl: 0,
            gio: 0,
            gio_chicken: 0,
            hpet: 0,
            link_pwr: 0,
            mbox: 0,
            shadow: 0,
            ucode_gp1: 0,
            ltr: 0,
            int_coalescing: 0,
            periodic: None,
            ctxt_ba: 0,
            ict: None,
            ict_cur: 0,
            int: 0,
            int_mask: 0,
            fh_int_status: 0,
            msix_fh: 0,
            msix_hw: 0,
            msix_fh_mask: !0,
            msix_hw_mask: !0,
            automasked: false,
            ivar: [0; 0x50],
            prph: HashMap::new(),
            prph_waddr: 0,
            prph_raddr: 0,
            mem_raddr: 0,
            sram: HashMap::new(),
            rx: None,
            backlog: VecDeque::new(),
            fw: None,
            master_stopped: false,
            air: Air::new(),
        };
        d.reset();
        d
    }

    /// The device's clock (GP2): microseconds.
    pub fn gp2(&self) -> u32 {
        self.start.elapsed().as_micros() as u32
    }

    /// The driver's SW_RESET: everything but the memory it gave and the
    /// air.
    pub fn reset(&mut self) {
        self.hw_if_config = 0;
        self.gp_cntrl = 0;
        (self.gio, self.gio_chicken, self.hpet, self.link_pwr, self.mbox, self.shadow) = (0, 0, 0, 0, 0, 0);
        self.ucode_gp1 = 0;
        self.ltr = 0;
        self.int_coalescing = 0;
        self.periodic = None;
        self.ctxt_ba = 0;
        self.ict = None;
        self.ict_cur = 0;
        (self.int, self.int_mask, self.fh_int_status) = (0, 0, 0);
        (self.msix_fh, self.msix_hw) = (0, 0);
        (self.msix_fh_mask, self.msix_hw_mask) = (!0, !0);
        self.automasked = false;
        self.ivar = [0; 0x50];
        self.prph.clear();
        self.sram.clear();
        self.rx = None;
        self.backlog.clear();
        self.fw = None;
        self.master_stopped = false;
        self.update_irq();
    }

    /// Stop at what a real device would not take (the first time).
    pub fn violation(&mut self, what: String) {
        if self.failed.is_some() {
            return;
        }
        eprintln!("{}: {what}", self.hw.name);
        self.failed = Some(what);
        if self.fw.is_some() {
            // The firmware's assert: its error id in the LMAC's log,
            // SW_ERR raised.
            self.sram.insert(iwx::sim::LMAC_ERROR_TABLE, 1);
            self.sram.insert(iwx::sim::LMAC_ERROR_TABLE + 4, ERROR_ID);
            self.raise(Cause::SwErr);
        }
    }

    fn awake(&self) -> bool {
        self.gp_cntrl & GP_MAC_ACCESS_REQ != 0 && self.clock_ready()
    }

    fn clock_ready(&self) -> bool {
        self.gp_cntrl & GP_INIT_DONE != 0
    }

    /// The device in MSI-X mode (UREG_CHICK), else MSI (and INTx).
    fn msix(&self) -> bool {
        self.prph.get(&(UREG_CHICK & PRPH_MASK)).is_some_and(|v| v & UREG_CHICK_MSIX_ENABLE != 0)
    }

    pub fn read(&mut self, off: u32, size: usize) -> u64 {
        match size {
            4 => u64::from(self.read32(off)),
            8 => u64::from(self.read32(off)) | u64::from(self.read32(off + 4)) << 32,
            1 | 2 => {
                let v = self.read32(off & !3) >> (8 * (off & 3));
                u64::from(v) & ((1u64 << (8 * size)) - 1)
            }
            _ => {
                self.violation(format!("a {size}-byte read at 0x{off:x}"));
                0
            }
        }
    }

    pub fn write(&mut self, off: u32, size: usize, val: u64) {
        match size {
            4 => self.write32(off, val as u32),
            8 => {
                self.write32(off, val as u32);
                self.write32(off + 4, (val >> 32) as u32);
            }
            1 => self.write8(off, val as u8),
            _ => self.violation(format!("a {size}-byte write of 0x{val:x} at 0x{off:x}")),
        }
    }

    fn read32(&mut self, off: u32) -> u32 {
        match off {
            HW_IF_CONFIG => self.hw_if_config,
            INT => self.int,
            INT_MASK => self.int_mask,
            FH_INT_STATUS => self.fh_int_status,
            RESET => if self.master_stopped { RESET_MASTER_DISABLED } else { 0 },
            // The radio is on (no kill switch).
            GP_CNTRL => self.gp_cntrl | if self.clock_ready() { GP_MAC_CLOCK_READY } else { 0 } | GP_HW_RF_KILL_SW,
            HW_REV => self.hw.hw_rev_raw,
            LTR_LONG_VAL_AD => self.ltr,
            GIO => self.gio,
            GIO_CHICKEN => self.gio_chicken,
            DBG_HPET_MEM => self.hpet,
            DBG_LINK_PWR_MGMT => self.link_pwr,
            MBOX_SET => self.mbox,
            MAC_SHADOW_CTRL => self.shadow,
            CTXT_INFO_BA => self.ctxt_ba as u32,
            o if o == CTXT_INFO_BA + 4 => (self.ctxt_ba >> 32) as u32,
            DRAM_INT_TBL => self.ict.map_or(0, |p| DRAM_INT_TBL_ENABLE | (p >> 12) as u32),
            o if (MAC_ADDR..MAC_ADDR + 16).contains(&o) && o % 4 == 0 => {
                if !self.awake() {
                    self.violation(format!("MAC address word 0x{o:x} read with the NIC asleep"));
                    return 0;
                }
                // The OTP's words; the strap's are empty.
                let m = MAC;
                match o - MAC_ADDR {
                    0 => u32::from_le_bytes([m[3], m[2], m[1], m[0]]),
                    4 => u32::from_le_bytes([m[5], m[4], 0, 0]),
                    _ => 0,
                }
            }
            HBUS_TARG_PRPH_RDAT => self.prph_read(),
            HBUS_TARG_MEM_RDAT => {
                if !self.awake() {
                    self.violation("device memory read with the NIC asleep".into());
                    return 0;
                }
                let v = self.sram.get(&self.mem_raddr).copied().unwrap_or(0);
                self.mem_raddr = self.mem_raddr.wrapping_add(4);
                v
            }
            MSIX_FH_INT_CAUSES => self.msix_fh,
            MSIX_HW_INT_CAUSES => self.msix_hw,
            MSIX_FH_INT_MASK => self.msix_fh_mask,
            MSIX_HW_INT_MASK => self.msix_hw_mask,
            MSIX_AUTOMASK_ST => u32::from(self.automasked),
            o if (MSIX_RX_IVAR..MSIX_RX_IVAR + 0x50).contains(&o) => {
                let i = (o - MSIX_RX_IVAR) as usize;
                u32::from_le_bytes(self.ivar[i..i + 4].try_into().unwrap_or([0; 4]))
            }
            o => {
                self.violation(format!("read of register 0x{o:x}, which the device does not have"));
                0
            }
        }
    }

    fn write32(&mut self, off: u32, v: u32) {
        match off {
            HW_IF_CONFIG => self.hw_if_config = v,
            INT => {
                self.int &= !v;
                self.update_irq();
            }
            INT_MASK => {
                self.int_mask = v;
                self.update_irq();
            }
            FH_INT_STATUS => self.fh_int_status &= !v,
            RESET => {
                if v & RESET_SW != 0 {
                    self.reset();
                } else if v & RESET_STOP_MASTER != 0 {
                    self.master_stopped = true;
                }
            }
            GP_CNTRL => {
                let rw = GP_INIT_DONE | GP_MAC_ACCESS_REQ | GP_RFKILL_WAKE_L1A_EN;
                self.gp_cntrl = v & rw;
            }
            GIO => self.gio = v,
            GIO_CHICKEN => self.gio_chicken = v,
            DBG_HPET_MEM => self.hpet = v,
            DBG_LINK_PWR_MGMT => self.link_pwr = v,
            MBOX_SET => self.mbox = v,
            MAC_SHADOW_CTRL => self.shadow = v,
            UCODE_DRV_GP1_CLR => self.ucode_gp1 &= !v,
            LTR_LONG_VAL_AD => self.ltr = v,
            CTXT_INFO_BA => self.ctxt_ba = self.ctxt_ba & !0xffff_ffff | u64::from(v),
            o if o == CTXT_INFO_BA + 4 => self.ctxt_ba = self.ctxt_ba & 0xffff_ffff | u64::from(v) << 32,
            DRAM_INT_TBL => {
                // The table's page number below the flags (bits 27, 28, 31).
                self.ict = (v & DRAM_INT_TBL_ENABLE != 0).then(|| u64::from(v & 0x07ff_ffff) << 12);
                self.ict_cur = 0;
            }
            HBUS_TARG_PRPH_WADDR | HBUS_TARG_PRPH_RADDR => {
                if v >> 24 != 3 {
                    self.violation(format!("periphery address 0x{v:x} not of a 32-bit word"));
                }
                if off == HBUS_TARG_PRPH_WADDR {
                    self.prph_waddr = v & PRPH_MASK;
                } else {
                    self.prph_raddr = v & PRPH_MASK;
                }
            }
            HBUS_TARG_PRPH_WDAT => self.prph_write(v),
            HBUS_TARG_MEM_RADDR => {
                if !self.awake() {
                    self.violation("device memory address set with the NIC asleep".into());
                }
                self.mem_raddr = v;
            }
            HBUS_TARG_WRPTR => {
                let (qid, idx) = ((v >> 16) as u8, (v & 0xffff) as usize);
                if idx >= CMD_RING {
                    self.violation(format!("queue {qid}'s write pointer {idx} past its ring"));
                    return;
                }
                super::firmware::kick(self, qid, idx);
            }
            RFH_Q0_FRBDCB_WIDX_TRG => {
                let Some(rx) = self.rx.as_mut() else {
                    self.violation("free receive buffers given before the firmware set its ring up".into());
                    return;
                };
                if v as usize >= RX_RING || v % 8 != 0 {
                    self.violation(format!("free receive buffer index {v}: not a multiple of 8 in the ring"));
                    return;
                }
                rx.widx = v as usize;
                self.flush_rx();
            }
            MSIX_FH_INT_CAUSES => {
                self.msix_fh &= !v;
                self.update_irq();
            }
            MSIX_HW_INT_CAUSES => {
                self.msix_hw &= !v;
                self.update_irq();
            }
            MSIX_FH_INT_MASK => {
                self.msix_fh_mask = v;
                self.update_irq();
            }
            MSIX_HW_INT_MASK => {
                self.msix_hw_mask = v;
                self.update_irq();
            }
            MSIX_AUTOMASK_ST => {
                if v & 1 != 0 {
                    self.automasked = false;
                    self.update_irq();
                }
            }
            o if (MSIX_RX_IVAR..MSIX_RX_IVAR + 0x50).contains(&o) => {
                let i = (o - MSIX_RX_IVAR) as usize;
                self.ivar[i..i + 4].copy_from_slice(&v.to_le_bytes());
            }
            o => self.violation(format!("write of 0x{v:x} to register 0x{o:x}, which the device does not have")),
        }
    }

    fn write8(&mut self, off: u32, v: u8) {
        match off {
            INT_COALESCING => self.int_coalescing = v,
            INT_PERIODIC => {
                self.periodic = match v {
                    INT_PERIODIC_ENA => Some(Instant::now() + PERIODIC),
                    INT_PERIODIC_DIS => None,
                    _ => {
                        self.violation(format!("INT_PERIODIC 0x{v:x}"));
                        None
                    }
                }
            }
            o if (MSIX_RX_IVAR..MSIX_RX_IVAR + 0x50).contains(&o) => {
                // Every cause to vector 0 (the only one), cleared by the
                // driver.
                if v & 0x7f >= MSIX_VECTORS as u8 {
                    self.violation(format!("MSI-X cause 0x{o:x} to vector {}", v & 0x7f));
                }
                self.ivar[(o - MSIX_RX_IVAR) as usize] = v;
            }
            o => self.violation(format!("byte write of 0x{v:x} to register 0x{o:x}")),
        }
    }

    fn prph_read(&mut self) -> u32 {
        let a = self.prph_raddr;
        // The persistence bit is read before the NIC is woken
        // (iwx_clear_persistence_bit): it reads clear.
        let early = a == HPM_DEBUG & PRPH_MASK || a == PREG_PRPH_WPROT_22000 & PRPH_MASK;
        if !early && !self.awake() {
            self.violation(format!("periphery 0x{a:x} read with the NIC asleep"));
            return 0;
        }
        match a {
            a if a == HPM_DEBUG & PRPH_MASK
                || a == PREG_PRPH_WPROT_22000 & PRPH_MASK
                || a == HPM_HIPM_GEN_CFG & PRPH_MASK
                || a == UREG_CHICK & PRPH_MASK =>
            {
                self.prph.get(&a).copied().unwrap_or(0)
            }
            a => {
                self.violation(format!("read of periphery 0x{a:x}, which the device does not have"));
                0
            }
        }
    }

    fn prph_write(&mut self, v: u32) {
        let a = self.prph_waddr;
        if !self.awake() {
            self.violation(format!("periphery 0x{a:x} written (0x{v:x}) with the NIC asleep"));
            return;
        }
        match a {
            a if a == HPM_DEBUG & PRPH_MASK
                || a == HPM_HIPM_GEN_CFG & PRPH_MASK
                || a == HPM_MAC_LTR_CSR & PRPH_MASK
                || a == HPM_UMAC_LTR & PRPH_MASK =>
            {
                self.prph.insert(a, v);
            }
            a if a == UREG_CHICK & PRPH_MASK => {
                if v & (UREG_CHICK_MSI_ENABLE | UREG_CHICK_MSIX_ENABLE) == UREG_CHICK_MSI_ENABLE | UREG_CHICK_MSIX_ENABLE {
                    self.violation("UREG_CHICK: both MSI and MSI-X".into());
                }
                self.prph.insert(a, v);
            }
            a if a == UREG_CPU_INIT_RUN & PRPH_MASK => {
                if v != 1 {
                    self.violation(format!("UREG_CPU_INIT_RUN 0x{v:x}"));
                    return;
                }
                if self.fw.is_some() {
                    self.violation("the firmware started twice without a reset".into());
                    return;
                }
                if let Err(e) = self.boot_ltr().and_then(|()| super::firmware::start(self, self.ctxt_ba)) {
                    self.violation(e);
                }
            }
            a => self.violation(format!("write of 0x{v:x} to periphery 0x{a:x}, which the device does not have")),
        }
    }

    /// The latency tolerance the self load needs set (the boot's
    /// workaround, iwx_set_ltr): snoop and no-snoop both required, where
    /// this radio takes it.
    fn boot_ltr(&self) -> Result<(), String> {
        const REQ: u32 = 1 << 31 | 1 << 15;
        let umac = self.prph.get(&(HPM_UMAC_LTR & PRPH_MASK)).copied().unwrap_or(0);
        let enabled = self.prph.get(&(HPM_MAC_LTR_CSR & PRPH_MASK)).copied().unwrap_or(0);
        let (ltr, other) = if self.hw.integrated { (umac, self.ltr) } else { (self.ltr, umac | enabled) };
        if ltr & REQ != REQ || other != 0 {
            let place = if self.hw.integrated { "the periphery's HPM_UMAC_LTR" } else { "LTR_LONG_VAL_AD" };
            return Err(format!("the self load started with the latency tolerance 0x{ltr:x}: the {} takes it in {place}", self.hw.name));
        }
        if self.hw.integrated && enabled != HPM_MAC_LRT_ENABLE_ALL {
            return Err(format!("the self load started with HPM_MAC_LTR_CSR 0x{enabled:x}"));
        }
        Ok(())
    }

    /// Raise a cause: in CSR_INT for MSI and INTx, the MSI-X cause
    /// registers in MSI-X mode.
    pub fn raise(&mut self, c: Cause) {
        if self.msix() {
            match c {
                Cause::Alive => self.msix_hw |= MSIX_HW_ALIVE,
                Cause::Rx => self.msix_fh |= MSIX_FH_Q0,
                Cause::SwErr => self.msix_hw |= MSIX_HW_SW_ERR,
            }
        } else {
            match c {
                Cause::Alive => self.int |= INT_ALIVE,
                Cause::Rx => {
                    self.int |= INT_FH_RX;
                    self.fh_int_status |= 1 << 16;
                }
                Cause::SwErr => self.int |= INT_SW_ERR,
            }
        }
        self.update_irq();
    }

    /// The interrupt's level, and the host told on its rise: an ICT
    /// entry first (MSI, INTx), the vector masked (MSI-X).
    fn update_irq(&mut self) {
        let msix = self.msix();
        let level = if msix {
            !self.automasked && (self.msix_fh & !self.msix_fh_mask != 0 || self.msix_hw & !self.msix_hw_mask != 0)
        } else {
            self.int & self.int_mask != 0
        };
        let rise = level && !self.level;
        self.level = level;
        if !rise {
            if !level {
                self.irq.set(false);
            }
            return;
        }
        if (self.irq.mode() == IrqMode::Msix) != msix {
            let dev = if msix { "MSI-X" } else { "MSI" };
            self.violation(format!("the device is in {dev} mode, the host takes its interrupt as {:?}", self.irq.mode()));
        }
        if msix {
            self.automasked = true;
            self.level = false;
            self.irq.set(true);
            self.irq.set(false);
            return;
        }
        if let Some(ict) = self.ict {
            let bits = self.int & self.int_mask;
            let entry = (bits & 0xff) | ((bits >> 16) & 0xff00);
            let at = ict + 4 * self.ict_cur as u64;
            if !self.mem.write(at, &entry.to_le_bytes()) {
                self.violation(format!("ICT entry at 0x{at:x}: memory the device was not given"));
                return;
            }
            self.ict_cur = (self.ict_cur + 1) % 1024;
        }
        self.irq.set(true);
    }

    /// The firmware's time: the periodic interrupt, then what the firmware
    /// does by itself. When to come back at the latest.
    pub fn tick(&mut self) -> Duration {
        let now = Instant::now();
        if self.periodic.is_some_and(|t| now >= t) {
            self.periodic = None;
            if !self.msix() {
                self.int |= INT_RX_PERIODIC;
                self.update_irq();
            }
        }
        let mut next = Duration::from_millis(20);
        if let Some(t) = self.periodic {
            next = next.min(t.saturating_duration_since(now));
        }
        if self.failed.is_none() && self.fw.is_some() {
            next = next.min(super::firmware::tick(self, now));
        }
        next
    }

    pub fn dma_read(&mut self, addr: u64, len: usize, what: &str) -> Option<Vec<u8>> {
        let mut b = vec![0; len];
        if self.mem.read(addr, &mut b) {
            return Some(b);
        }
        self.violation(format!("{what} at 0x{addr:x} (+{len}): memory the device was not given"));
        None
    }

    pub fn dma_write(&mut self, addr: u64, data: &[u8], what: &str) -> bool {
        if self.mem.write(addr, data) {
            return true;
        }
        self.violation(format!("{what} at 0x{addr:x} (+{}): memory the device was not given", data.len()));
        false
    }

    /// A packet to the host: {code, group, idx, qid}, then `data`, in the
    /// next free receive buffer (or waiting for one). `answer` keeps it
    /// from being dropped when the backlog is full.
    pub fn send(&mut self, hdr: [u8; 4], data: &[u8], answer: bool) {
        if self.rx.is_none() {
            self.violation("a packet to send before the receive ring is set up".into());
            return;
        }
        if self.backlog.len() >= BACKLOG && !answer {
            return;
        }
        let mut p = Vec::with_capacity(8 + data.len());
        p.extend_from_slice(&((4 + data.len()) as u32).to_le_bytes());
        p.extend_from_slice(&hdr);
        p.extend_from_slice(data);
        self.backlog.push_back(p);
        self.flush_rx();
    }

    /// The packets waiting, each into a free buffer while there is one
    /// (iwx_rx_pkt reads them back): FRAME_INVALID after it, the used
    /// descriptor and the closed count written, then the RX cause.
    fn flush_rx(&mut self) {
        let mut sent = false;
        while self.failed.is_none() && self.rx.as_ref().is_some_and(|r| r.free() > 0) && !self.backlog.is_empty() {
            let p = self.backlog.pop_front().unwrap();
            let rx = self.rx.as_ref().unwrap();
            let (pos, free_rbd, used_rbd, status) = (rx.pos, rx.free_rbd, rx.used_rbd, rx.status);
            let Some(d) = self.dma_read(free_rbd + 8 * pos as u64, 8, "free receive buffer descriptor") else {
                return;
            };
            let d = le64(&d, 0);
            let (buf, vid) = (d & !0xfff, (d & 0xfff) as u32);
            if vid as usize >= RX_RING {
                self.violation(format!("free receive buffer descriptor {pos}: index {vid} past the ring"));
                return;
            }
            let mut b = p;
            if b.len() > RX_BUF {
                self.violation(format!("a {}-byte packet does not fit a receive buffer", b.len()));
                return;
            }
            let end = b.len().next_multiple_of(64);
            if end + 4 <= RX_BUF {
                b.resize(end, 0);
                b.extend_from_slice(&FRAME_INVALID.to_le_bytes());
            }
            if !self.dma_write(buf, &b, "receive buffer") || !self.dma_write(used_rbd + 4 * pos as u64, &vid.to_le_bytes(), "used receive buffer descriptor") {
                return;
            }
            let rx = self.rx.as_mut().unwrap();
            rx.pos = (pos + 1) % RX_RING;
            rx.closed = (rx.closed + 1) % 4096;
            let closed = rx.closed;
            if !self.dma_write(status, &closed.to_le_bytes(), "receive ring status") {
                return;
            }
            sent = true;
        }
        if sent {
            self.raise(Cause::Rx);
        }
    }
}

/// The id the firmware's log carries when the model stopped it: "bad"
/// (`failed` says why).
pub const ERROR_ID: u32 = 0x0000_0bad;

pub fn le16(b: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([b[at], b[at + 1]])
}

pub fn le32(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(b[at..at + 4].try_into().unwrap())
}

pub fn le64(b: &[u8], at: usize) -> u64 {
    u64::from_le_bytes(b[at..at + 8].try_into().unwrap())
}

pub fn put16(b: &mut [u8], at: usize, v: u16) {
    b[at..at + 2].copy_from_slice(&v.to_le_bytes());
}

pub fn put32(b: &mut [u8], at: usize, v: u32) {
    b[at..at + 4].copy_from_slice(&v.to_le_bytes());
}
