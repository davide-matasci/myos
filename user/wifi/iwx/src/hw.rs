//! Register access and bring-up: the NIC lock, the periphery window,
//! preparing and starting the hardware, its interrupt masks (from OpenBSD's
//! if_iwx.c: iwx_nic_lock, iwx_prepare_card_hw, iwx_start_hw, iwx_apm_init,
//! iwx_conf_msix_hw and their helpers; notices in LICENSE). For the 22000
//! family (AX200/AX201) only.

use crate::reg::*;

/// The device as the driver reaches it: its registers (BAR0) and time.
pub trait Bus {
    fn read32(&mut self, off: u32) -> u32;
    fn write32(&mut self, off: u32, val: u32);
    fn write8(&mut self, off: u32, val: u8);
    /// Busy-wait (the waits here are microseconds to tens of
    /// milliseconds, as the C driver's DELAY).
    fn delay_us(&mut self, us: u32);
}

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum HwError {
    /// The device never said it was ready (iwx_prepare_card_hw).
    NotReady,
    /// The MAC clock never settled.
    Clock,
    /// The NIC lock could not be taken (the device asleep, or gone).
    Lock,
    /// The persistence bit is set and write-protected.
    Persistence,
}

/// The hardware: its bus, and what the driver tracks of it.
pub struct Hw<B: Bus> {
    pub bus: B,
    /// Interrupts through MSI-X (else MSI).
    pub msix: bool,
    /// Integrated (CNVi) part, the AX201's (iwx_attach's sc_integrated), or
    /// a discrete one, the AX200's.
    pub integrated: bool,
    locks: u32,
    /// MSI: the causes INT_MASK lets through.
    pub intmask: u32,
    /// MSI-X: the causes enabled at start (as read back), and now.
    pub fh_init_mask: u32,
    pub hw_init_mask: u32,
    pub fh_mask: u32,
    pub hw_mask: u32,
}

impl<B: Bus> Hw<B> {
    pub fn new(bus: B, msix: bool, integrated: bool) -> Self {
        Hw { bus, msix, integrated, locks: 0, intmask: 0, fh_init_mask: 0, hw_init_mask: 0, fh_mask: 0, hw_mask: 0 }
    }

    pub fn read(&mut self, off: u32) -> u32 {
        self.bus.read32(off)
    }

    pub fn write(&mut self, off: u32, val: u32) {
        self.bus.write32(off, val)
    }

    pub fn set_bits(&mut self, off: u32, bits: u32) {
        let v = self.read(off);
        self.write(off, v | bits);
    }

    pub fn clear_bits(&mut self, off: u32, bits: u32) {
        let v = self.read(off);
        self.write(off, v & !bits);
    }

    /// Wait up to `timeout_us` for `reg & mask == bits & mask`, checking
    /// every 10 µs (iwx_poll_bit).
    pub fn poll_bit(&mut self, reg: u32, bits: u32, mask: u32, timeout_us: u32) -> bool {
        let mut left = timeout_us;
        loop {
            if self.read(reg) & mask == bits & mask {
                return true;
            }
            if left < 10 {
                return false;
            }
            left -= 10;
            self.bus.delay_us(10);
        }
    }

    /// Wake the device for access to its periphery and memory; counted
    /// (iwx_nic_lock).
    pub fn nic_lock(&mut self) -> Result<(), HwError> {
        if self.locks > 0 {
            self.locks += 1;
            return Ok(());
        }
        self.set_bits(GP_CNTRL, GP_MAC_ACCESS_REQ);
        self.bus.delay_us(2);
        if self.poll_bit(GP_CNTRL, GP_MAC_CLOCK_READY, GP_MAC_CLOCK_READY | GP_GOING_TO_SLEEP, 150_000) {
            self.locks = 1;
            Ok(())
        } else {
            Err(HwError::Lock)
        }
    }

    pub fn nic_unlock(&mut self) {
        if self.locks > 0 {
            self.locks -= 1;
            if self.locks == 0 {
                self.clear_bits(GP_CNTRL, GP_MAC_ACCESS_REQ);
            }
        }
    }

    pub fn read_prph_unlocked(&mut self, addr: u32) -> u32 {
        self.write(HBUS_TARG_PRPH_RADDR, (addr & PRPH_MASK) | (3 << 24));
        self.read(HBUS_TARG_PRPH_RDAT)
    }

    pub fn write_prph_unlocked(&mut self, addr: u32, val: u32) {
        self.write(HBUS_TARG_PRPH_WADDR, (addr & PRPH_MASK) | (3 << 24));
        self.write(HBUS_TARG_PRPH_WDAT, val);
    }

    /// A periphery write under the NIC lock.
    pub fn write_prph(&mut self, addr: u32, val: u32) -> Result<(), HwError> {
        self.nic_lock()?;
        self.write_prph_unlocked(addr, val);
        self.nic_unlock();
        Ok(())
    }

    /// `reg = reg & mask | bits` in the periphery (iwx_set_bits_mask_prph).
    pub fn set_bits_mask_prph(&mut self, addr: u32, bits: u32, mask: u32) -> Result<(), HwError> {
        self.nic_lock()?;
        let v = self.read_prph_unlocked(addr) & mask;
        self.write_prph_unlocked(addr, v | bits);
        self.nic_unlock();
        Ok(())
    }

    /// `dwords` words of the device's memory from `addr` (its error logs).
    pub fn read_mem(&mut self, addr: u32, out: &mut [u32]) -> Result<(), HwError> {
        self.nic_lock()?;
        self.write(HBUS_TARG_MEM_RADDR, addr);
        for w in out.iter_mut() {
            *w = self.read(HBUS_TARG_MEM_RDAT);
        }
        self.nic_unlock();
        Ok(())
    }

    fn set_hw_ready(&mut self) -> bool {
        self.set_bits(HW_IF_CONFIG, HW_IF_NIC_READY);
        let ready = self.poll_bit(HW_IF_CONFIG, HW_IF_NIC_READY, HW_IF_NIC_READY, 50);
        if ready {
            self.set_bits(MBOX_SET, MBOX_OS_ALIVE);
        }
        ready
    }

    /// Up to 10 tries of 150 ms, the device woken in between.
    pub fn prepare_card_hw(&mut self) -> Result<(), HwError> {
        if self.set_hw_ready() {
            return Ok(());
        }
        self.set_bits(DBG_LINK_PWR_MGMT, RESET_LINK_PWR_MGMT_DISABLED);
        self.bus.delay_us(1000);
        let mut t = 0;
        for _ in 0..10 {
            self.set_bits(HW_IF_CONFIG, HW_IF_PREPARE);
            loop {
                if self.set_hw_ready() {
                    return Ok(());
                }
                self.bus.delay_us(200);
                t += 200;
                if t >= 150_000 {
                    break;
                }
            }
            self.bus.delay_us(25_000);
        }
        Err(HwError::NotReady)
    }

    fn clear_persistence_bit(&mut self) -> Result<(), HwError> {
        let hpm = self.read_prph_unlocked(HPM_DEBUG);
        if hpm != 0xa5a5_a5a0 && hpm & PERSISTENCE_BIT != 0 {
            if self.read_prph_unlocked(PREG_PRPH_WPROT_22000) & PREG_WFPM_ACCESS != 0 {
                return Err(HwError::Persistence);
            }
            self.write_prph_unlocked(HPM_DEBUG, hpm & !PERSISTENCE_BIT);
        }
        Ok(())
    }

    fn sw_reset(&mut self) {
        self.set_bits(RESET, RESET_SW);
        self.bus.delay_us(5000);
    }

    fn force_power_gating(&mut self) -> Result<(), HwError> {
        self.set_bits_mask_prph(HPM_HIPM_GEN_CFG, HIPM_FORCE_ACTIVE, !0)?;
        self.bus.delay_us(20);
        self.set_bits_mask_prph(HPM_HIPM_GEN_CFG, HIPM_PG_EN | HIPM_SLP_EN, !0)?;
        self.bus.delay_us(20);
        self.set_bits_mask_prph(HPM_HIPM_GEN_CFG, 0, !HIPM_FORCE_ACTIVE)
    }

    /// Power the NIC's basic functions up after a reset; loads nothing
    /// (iwx_apm_init). L0s stays off, as the C driver keeps it.
    pub fn apm_init(&mut self) -> Result<(), HwError> {
        self.set_bits(GIO_CHICKEN, GIO_CHICKEN_L1A_NO_L0S_RX);
        self.set_bits(DBG_HPET_MEM, DBG_HPET_MEM_VAL);
        self.set_bits(HW_IF_CONFIG, HW_IF_HAP_WAKE_L1A);
        self.set_bits(GIO, GIO_L0S_DISABLED);
        self.set_bits(GP_CNTRL, GP_INIT_DONE);
        if self.poll_bit(GP_CNTRL, GP_MAC_CLOCK_READY, GP_MAC_CLOCK_READY, 25_000) {
            Ok(())
        } else {
            Err(HwError::Clock)
        }
    }

    /// Prepare, reset and power the device up, its interrupt routed
    /// (iwx_start_hw).
    pub fn start_hw(&mut self) -> Result<(), HwError> {
        self.prepare_card_hw()?;
        self.clear_persistence_bit()?;
        self.sw_reset();
        if self.integrated {
            self.set_bits(GP_CNTRL, GP_INIT_DONE);
            self.bus.delay_us(20);
            if !self.poll_bit(GP_CNTRL, GP_MAC_CLOCK_READY, GP_MAC_CLOCK_READY, 25_000) {
                return Err(HwError::Clock);
            }
            self.force_power_gating()?;
            self.sw_reset();
        }
        self.apm_init()?;
        self.init_msix_hw()?;
        self.enable_rfkill_int();
        Ok(())
    }

    /// Route the interrupt: MSI, or every cause to MSI-X vector 0
    /// (iwx_init_msix_hw, iwx_conf_msix_hw).
    fn init_msix_hw(&mut self) -> Result<(), HwError> {
        if !self.msix {
            return self.write_prph(UREG_CHICK, UREG_CHICK_MSI_ENABLE);
        }
        self.write_prph(UREG_CHICK, UREG_CHICK_MSIX_ENABLE)?;
        self.write(MSIX_FH_INT_MASK, !0);
        self.write(MSIX_HW_INT_MASK, !0);
        // The command/management queue and the data one, then the other
        // causes, all to vector 0, cleared by the driver.
        self.bus.write8(MSIX_RX_IVAR, MSIX_NON_AUTO_CLEAR);
        self.bus.write8(MSIX_RX_IVAR + 1, MSIX_NON_AUTO_CLEAR);
        self.clear_bits(MSIX_FH_INT_MASK, MSIX_FH_Q0 | MSIX_FH_Q1);
        for cause in MSIX_IVAR_CAUSES {
            self.bus.write8(MSIX_IVAR + cause, MSIX_NON_AUTO_CLEAR);
        }
        self.clear_bits(MSIX_FH_INT_MASK, MSIX_FH_D2S_CH0 | MSIX_FH_D2S_CH1 | MSIX_FH_S2D | MSIX_FH_ERR);
        self.clear_bits(
            MSIX_HW_INT_MASK,
            MSIX_HW_ALIVE
                | MSIX_HW_WAKEUP
                | MSIX_HW_RESET_DONE
                | MSIX_HW_CT_KILL
                | MSIX_HW_RF_KILL
                | MSIX_HW_PERIODIC
                | MSIX_HW_SW_ERR
                | MSIX_HW_SW_ERR_V2
                | MSIX_HW_SCD
                | MSIX_HW_FH_TX
                | MSIX_HW_HW_ERR
                | MSIX_HW_HAP,
        );
        self.fh_init_mask = !self.read(MSIX_FH_INT_MASK);
        self.hw_init_mask = !self.read(MSIX_HW_INT_MASK);
        self.fh_mask = self.fh_init_mask;
        self.hw_mask = self.hw_init_mask;
        Ok(())
    }

    fn enable_rfkill_int(&mut self) {
        if self.msix {
            self.write(MSIX_FH_INT_MASK, self.fh_init_mask);
            self.write(MSIX_HW_INT_MASK, !MSIX_HW_RF_KILL);
            self.hw_mask = MSIX_HW_RF_KILL;
        } else {
            self.intmask = INT_RF_KILL;
            self.write(INT_MASK, self.intmask);
        }
        self.set_bits(GP_CNTRL, GP_RFKILL_WAKE_L1A_EN);
    }

    /// Whether the radio is switched off (the platform's kill switch);
    /// the bit is set when it is on.
    pub fn rfkill(&mut self) -> bool {
        self.read(GP_CNTRL) & GP_HW_RF_KILL_SW == 0
    }

    pub fn enable_interrupts(&mut self) {
        if self.msix {
            self.hw_mask = self.hw_init_mask;
            self.fh_mask = self.fh_init_mask;
            self.write(MSIX_FH_INT_MASK, !self.fh_mask);
            self.write(MSIX_HW_INT_MASK, !self.hw_mask);
        } else {
            self.intmask = INT_INI_SET;
            self.write(INT_MASK, self.intmask);
        }
    }

    /// Only ALIVE and receive while the firmware loads.
    pub fn enable_fwload_interrupt(&mut self) {
        if self.msix {
            self.write(MSIX_HW_INT_MASK, !MSIX_HW_ALIVE);
            self.hw_mask = MSIX_HW_ALIVE;
            self.write(MSIX_FH_INT_MASK, !self.fh_init_mask);
            self.fh_mask = self.fh_init_mask;
        } else {
            self.intmask = INT_ALIVE | INT_FH_RX;
            self.write(INT_MASK, self.intmask);
        }
    }

    pub fn disable_interrupts(&mut self) {
        if self.msix {
            self.write(MSIX_FH_INT_MASK, self.fh_init_mask);
            self.write(MSIX_HW_INT_MASK, self.hw_init_mask);
        } else {
            self.write(INT_MASK, 0);
            self.write(INT, !0);
            self.write(FH_INT_STATUS, !0);
        }
    }

    /// The MAC and radio steppings into HW_IF_CONFIG (iwx_nic_config).
    pub fn nic_config(&mut self, hw_rev: u32, phy_sku: u32) {
        let radio_type = phy_sku & 0x3;
        let radio_step = (phy_sku >> 2) & 0x3;
        let radio_dash = (phy_sku >> 4) & 0x3;
        let val = ((hw_rev & 0xc) >> 2) << HW_IF_MAC_STEP_POS
            | (hw_rev & 0x3) << HW_IF_MAC_DASH_POS
            | radio_type << HW_IF_PHY_TYPE_POS
            | radio_step << HW_IF_PHY_STEP_POS
            | radio_dash << HW_IF_PHY_DASH_POS;
        let mask = HW_IF_MAC_DASH
            | HW_IF_MAC_STEP
            | HW_IF_PHY_STEP
            | HW_IF_PHY_DASH
            | HW_IF_PHY_TYPE
            | HW_IF_RADIO_SI
            | HW_IF_MAC_SI;
        let v = self.read(HW_IF_CONFIG) & !mask;
        self.write(HW_IF_CONFIG, v | val);
    }

    /// Before the firmware load (iwx_nic_init, iwx_nic_rx_init and
    /// iwx_start_fw's preamble).
    pub fn nic_init(&mut self, hw_rev: u32, phy_sku: u32) -> Result<(), HwError> {
        self.write(INT, !0);
        self.disable_interrupts();
        self.write(UCODE_DRV_GP1_CLR, UCODE_SW_BIT_RFKILL);
        self.write(UCODE_DRV_GP1_CLR, UCODE_DRV_GP1_CMD_BLOCKED);
        self.write(INT, !0);
        self.apm_init()?;
        self.nic_config(hw_rev, phy_sku);
        self.bus.write8(INT_COALESCING, 0x40);
        self.set_bits(MAC_SHADOW_CTRL, MAC_SHADOW_CTRL_VAL);
        Ok(())
    }

    /// Hand the device its context information and start the firmware's
    /// self load (the end of iwx_ctxt_info_init, iwx_set_ltr): the address
    /// in two 32-bit writes (one 64-bit one fails on the AX201).
    pub fn kick_self_load(&mut self, ctxt_phys: u64) -> Result<(), HwError> {
        self.write(CTXT_INFO_BA, ctxt_phys as u32);
        self.write(CTXT_INFO_BA + 4, (ctxt_phys >> 32) as u32);
        // ~250 µs snoop and no-snoop latency until the firmware sets its
        // own: in a register of a discrete radio's, in the periphery of an
        // integrated one.
        if !self.integrated {
            self.write(LTR_LONG_VAL_AD, LTR_250_US);
        }
        self.nic_lock()?;
        if self.integrated {
            self.write_prph_unlocked(HPM_MAC_LTR_CSR, HPM_MAC_LRT_ENABLE_ALL);
            self.write_prph_unlocked(HPM_UMAC_LTR, LTR_250_US);
        }
        self.write_prph_unlocked(UREG_CPU_INIT_RUN, 1);
        self.nic_unlock();
        Ok(())
    }

    /// The ICT (the interrupt cause table in host memory) at `phys` (4 KB
    /// aligned) from now on (iwx_ict_reset).
    pub fn ict_reset(&mut self, phys: u64) {
        self.disable_interrupts();
        self.write(
            DRAM_INT_TBL,
            DRAM_INT_TBL_ENABLE | DRAM_INT_TBL_WRAP_CHECK | DRAM_INT_TBL_WRITE_POINTER | (phys >> 12) as u32,
        );
        self.write(INT, !0);
        self.enable_interrupts();
    }

    /// The MAC address (iwx_set_mac_addr_from_csr): the strap one, else the
    /// OTP one; each word's bytes reversed.
    pub fn mac_addr(&mut self) -> Result<[u8; 6], HwError> {
        self.nic_lock()?;
        let strap = (self.read(MAC_ADDR + 8), self.read(MAC_ADDR + 0xc));
        let otp = (self.read(MAC_ADDR), self.read(MAC_ADDR + 4));
        self.nic_unlock();
        let mac = mac_from_words(strap.0, strap.1);
        Ok(if valid_mac(&mac) { mac } else { mac_from_words(otp.0, otp.1) })
    }
}

/// The latency tolerance (LTR_LONG_VAL_AD, HPM_UMAC_LTR): no-snoop and snoop latency both required, 250 in
/// units of 1 µs (scale 2) (iwx_set_ltr).
pub const LTR_250_US: u32 = 0x8000_0000 | (2 << 24) & 0x1c00_0000 | (250 << 16) & 0x03ff_0000 | 0x8000 | (2 << 8) & 0x1c00 | 250;

/// A MAC address from the two words it is kept in (iwx_flip_hw_address).
pub fn mac_from_words(lo: u32, hi: u32) -> [u8; 6] {
    let a = lo.to_le_bytes();
    let b = hi.to_le_bytes();
    [a[3], a[2], a[1], a[0], b[1], b[0]]
}

/// Not zero, not a group address, not the one Intel reserves
/// (iwx_is_valid_mac_addr).
pub fn valid_mac(mac: &[u8; 6]) -> bool {
    mac != &[0; 6] && mac[0] & 1 == 0 && mac != &[0x02, 0xcc, 0xaa, 0xff, 0xee, 0x00]
}
