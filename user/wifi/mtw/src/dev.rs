//! The radio: its registers through vendor requests, the eFUSE ROM, the
//! firmware, the MAC, baseband and RF bring-up, channels, joining and
//! sending (OpenBSD's if_mtw.c: mtw_attach, mtw_attachhook, mtw_init,
//! mtw_set_chan, mtw_newstate's RUN state, mtw_stop and their helpers;
//! notices in LICENSE). For the MT7601U.

use crate::chip::Chip;
use crate::fw::Firmware;
use crate::rate::{Amrr, RateSet, TxRate};
use crate::reg::*;
use crate::tx::{self, Queue};
use crate::usb::{Endpoints, Usb, UsbError};

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum Error {
    /// A transfer failed.
    Usb,
    /// The device never got to where the driver waited for.
    Timeout(&'static str),
    /// ASIC_VER says it is another chip.
    Chip(u16),
    Firmware(&'static str),
    /// Not a channel the radio has.
    Channel(u8),
}

impl From<UsbError> for Error {
    fn from(_: UsbError) -> Self {
        Error::Usb
    }
}

pub type Result<T> = core::result::Result<T, Error>;

/// The access point joined, as its beacon describes it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Peer {
    pub bssid: [u8; 6],
    pub channel: u8,
    /// Its legacy rates (Supported Rates bytes: 500 kb/s units, the basic
    /// ones with bit 7 set).
    pub rates: alloc::vec::Vec<u8>,
    /// Its HT Capabilities' first MCS bitmap byte, when it is an 802.11n
    /// access point.
    pub ht_mcs: Option<u8>,
    /// Its capabilities: short preambles, short slots.
    pub short_preamble: bool,
    pub short_slot: bool,
}

/// What the radio says of itself once attached.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub struct Info {
    pub mac: [u8; 6],
    pub mac_ver: u16,
    pub mac_rev: u16,
    pub rf_rev: u16,
    /// Antennas: transmit and receive chains.
    pub ntx: u8,
    pub nrx: u8,
    /// The firmware's build and version, `None` when it was running
    /// already (the device was not reset since a daemon loaded it).
    pub fw: Option<(u16, u16)>,
}

/// The eFUSE ROM's calibration the driver uses.
#[derive(Default)]
struct Eeprom {
    freq_offset: u8,
    /// Each channel's transmit power.
    txpow1: [i8; 14],
    /// TX_PWR_CFG0 to 4: the power per rate group.
    txpow20mhz: [u32; 5],
    rssi_2ghz: i8,
    lna: u8,
}

pub struct Dev<U: Usb> {
    pub usb: U,
    pub chip: &'static Chip,
    eps: Endpoints,
    mac_ver: u16,
    ee: Eeprom,
    /// The channel group set up (sc_chan_group, sc_bw_calibrated).
    chan_group: Option<u8>,
    rf_calibrated: bool,
    /// The station's table entry for the access point joined, the rates
    /// data frames to it go at, and its preamble.
    wcid: u8,
    amrr: Option<Amrr>,
    short_preamble: bool,
}

impl<U: Usb> Dev<U> {
    /// The radio `chip` behind `usb` with bulk endpoints `eps`, once its
    /// ASIC answers (mtw_attach).
    pub fn new(mut usb: U, chip: &'static Chip, eps: Endpoints) -> Result<Self> {
        let mut ver = 0;
        for _ in 0..100 {
            ver = read(&mut usb, ASIC_VER)?;
            if ver != 0 && ver != 0xffff_ffff {
                break;
            }
            usb.delay_us(10);
        }
        if ver == 0 || ver == 0xffff_ffff {
            return Err(Error::Timeout("the NIC to initialize"));
        }
        if (ver >> 16) as u16 != chip.asic_ver {
            return Err(Error::Chip((ver >> 16) as u16));
        }
        Ok(Dev {
            usb,
            chip,
            eps,
            mac_ver: 0,
            ee: Eeprom::default(),
            chan_group: None,
            rf_calibrated: false,
            wcid: 0xff,
            amrr: None,
            short_preamble: false,
        })
    }

    /// Power the WLAN core, load and start `fw`, read the MAC version and
    /// the eFUSE ROM (mtw_attachhook).
    pub fn attach(&mut self, fw: &Firmware) -> Result<Info> {
        self.wlan_enable()?;
        let loaded = self.load_microcode(fw)?;
        let tmp = self.read(USB_DMA_CFG)?;
        self.write(USB_DMA_CFG, tmp | USB_RX_EN | USB_TX_EN)?;
        let mut ver = 0;
        for _ in 0..100 {
            ver = self.read(MAC_VER_ID)?;
            if ver != 0 && ver != 0xffff_ffff {
                break;
            }
            self.usb.delay_us(10);
        }
        if ver == 0 || ver == 0xffff_ffff {
            return Err(Error::Timeout("the MAC version"));
        }
        self.mac_ver = (ver >> 16) as u16;
        let mut info = self.read_eeprom()?;
        (info.mac_ver, info.mac_rev) = (self.mac_ver, ver as u16);
        info.fw = loaded.then_some((fw.build_ver, fw.fw_ver));
        Ok(info)
    }

    /// Bring the MAC, baseband and RF up for station `mac`, on channel 1,
    /// hearing and sending (mtw_init).
    pub fn init(&mut self, mac: [u8; 6]) -> Result<()> {
        let mut tmp = self.wait_dma_idle(100)?;
        tmp &= 0xff0;
        tmp |= TX_WB_DDONE;
        self.write(WPDMA_GLO_CFG, tmp)?;

        // Reset the MAC and the baseband.
        self.write(MAC_SYS_CTRL, BBP_HRST | MAC_SRST)?;
        self.write(USB_DMA_CFG, 0)?;
        self.write(MAC_SYS_CTRL, 0)?;
        for &(reg, val) in &DEF_MAC {
            self.write(reg, val)?;
        }
        self.wait(MAC_STATUS_REG, RX_STATUS_BUSY | TX_STATUS_BUSY, 100, 1000, "the MAC")?;

        self.set_macaddr(mac)?;
        // Clear the WCID attributes, RTS, the host to MCU mailbox, the
        // receive WCID table, TSF sync.
        self.set_region_4(WCID_ATTR0, 1, 8 * 32)?;
        self.write(0x1648, 0x0083_0083)?;
        let tmp = self.read(FCE_L2_STUFF)?;
        self.write(FCE_L2_STUFF, tmp & !L2S_WR_MPDU_LEN_EN)?;
        self.set_txrts()?;
        self.write(BBP_CSR, 0)?;
        self.write(H2M_MAILBOX, 0)?;
        self.set_region_4(WCID_ENTRY0, 0xffff_ffff, 512)?;
        self.abort_tsf_sync()?;
        let tmp = self.read(US_CYC_CNT)?;
        self.write(US_CYC_CNT, (tmp & !0xff) | 0x1e)?;
        // Clear the key tables: the daemon keeps the keys.
        self.set_region_4(SKEY0, 0, 8 * 32)?;
        self.set_region_4(IVEIV0, 0, 8 * 32)?;
        self.write(SKEY_MODE_0_7, 0)?;
        self.write(SKEY_MODE_8_15, 0)?;
        self.write(TXOP_CTRL_CFG, 0x0000_583f)?;

        // The transmit power of each rate group.
        for ridx in 0..5 {
            let p = self.ee.txpow20mhz[ridx];
            if p != 0xffff_ffff {
                self.write(TX_PWR_CFG0 + 4 * ridx as u16, p)?;
            }
        }
        self.write(TX_PWR_CFG7, 0)?;
        self.write(TX_PWR_CFG9, 0)?;
        let tmp = self.read(CMB_CTRL)?;
        self.write(CMB_CTRL, tmp & !(1 << 18 | 1 << 14))?;

        // Clear the USB DMA.
        self.write(USB_DMA_CFG, USB_TX_EN | USB_RX_EN | USB_RX_AGG_EN | USB_TX_CLEAR | USB_TXOP_HALT | USB_RX_WL_DROP)?;
        self.usb.delay_us(50_000);
        let tmp = self.read(USB_DMA_CFG)?;
        self.write(USB_DMA_CFG, tmp & !(USB_TX_CLEAR | USB_TXOP_HALT | USB_RX_WL_DROP))?;

        // The radio on, then RF, baseband, calibration.
        self.mcu_radio(0x31, 0)?;
        self.rf_init()?;
        self.bbp_init()?;
        self.rf_setup()?;

        self.set_chan(1)?;
        self.txrx_enable()?;
        self.set_leds(LED_MODE_ON)
    }

    /// Hear and send on 2.4 GHz channel `chan` (mtw_set_chan,
    /// mt7601_set_chan).
    pub fn set_chan(&mut self, chan: u8) -> Result<()> {
        let Some(i) = RF_CHAN.iter().position(|c| c.0 == chan) else {
            return Err(Error::Channel(chan));
        };
        if self.chan_group != Some(0) {
            self.select_chan_group(0)?;
        }
        self.chan_group = Some(0);

        let (_, r17, r18, r19, r20) = RF_CHAN[i];
        self.rf_write(0, 17, r17)?;
        self.rf_write(0, 18, r18)?;
        self.rf_write(0, 19, r19)?;
        self.rf_write(0, 20, r20)?;

        // Transmit automatic level control, from the ROM's power.
        let tmp = self.read(TX_ALC_CFG0)?;
        self.write(TX_ALC_CFG0, (tmp & !0x3f3f) | (self.ee.txpow1[i] as u32 & 0x3f))?;

        // LNA.
        let lna = 0x37u8.wrapping_sub(self.ee.lna);
        for reg in 62..=64 {
            self.bbp_write(reg, lna)?;
        }

        // VCO calibration.
        self.rf_write(0, 4, 0x0a)?;
        self.rf_write(0, 5, 0x20)?;
        let rf = self.rf_read(0, 4)?;
        self.rf_write(0, 4, rf | 0x80)?;

        self.select_20mhz()?;
        self.bbp_write(178, 0xff)?;
        self.usb.delay_us(1000);
        Ok(())
    }

    /// About to authenticate with `peer`: on its channel, its address the
    /// MAC's BSSID.
    pub fn joining(&mut self, peer: &Peer) -> Result<()> {
        self.set_chan(peer.channel)?;
        self.set_bssid(peer.bssid)
    }

    /// Associated to `peer` as `aid`, over 802.11n when `ht` (the
    /// association asked for it), as mtw_newstate_cb's RUN state and
    /// mtw_newassoc set the station up: the slot time, rate fallback,
    /// preamble and basic rates of the network, the access point in the
    /// station table, its rates for AMRR, which starts at the slowest.
    /// The beacon timer (TSF sync) is left off: the station does not
    /// sleep.
    pub fn associated(&mut self, peer: &Peer, aid: u16, ht: bool) -> Result<()> {
        let tmp = self.read(BKOFF_SLOT_CFG)?;
        self.write(BKOFF_SLOT_CFG, (tmp & !0xff) | if peer.short_slot { 9 } else { 20 })?;
        self.enable_mrr()?;
        let tmp = self.read(AUTO_RSP_CFG)?;
        let tmp = if peer.short_preamble { tmp | CCK_SHORT_EN } else { tmp & !CCK_SHORT_EN };
        self.write(AUTO_RSP_CFG, tmp)?;
        self.write(LEGACY_BASIC_RATE, basic_rates(&peer.rates))?;
        self.set_bssid(peer.bssid)?;
        self.wcid = aid_to_wcid(aid);
        self.write_region_1(WCID_ENTRY0 + 8 * u16::from(self.wcid), &peer.bssid)?;
        self.short_preamble = peer.short_preamble;
        self.amrr = Some(Amrr::new(RateSet::new(&peer.rates, peer.ht_mcs.filter(|_| ht))));
        // The counters start from the association.
        self.tx_stats()?;
        self.set_leds(LED_MODE_BLINK_TX)
    }

    /// Once a second while associated: the last second's TX statistics to
    /// AMRR, which may change the rate (mtw_calibrate_cb).
    pub fn calibrate(&mut self) -> Result<()> {
        if self.amrr.is_none() {
            return Ok(());
        }
        let (tx, retries) = self.tx_stats()?;
        if let Some(a) = self.amrr.as_mut() {
            a.update(tx, retries);
        }
        Ok(())
    }

    /// The rate data frames to the access point go at now (1 Mb/s when
    /// not associated).
    pub fn tx_rate(&self) -> TxRate {
        self.amrr.as_ref().map_or(TxRate::Legacy(RIDX_CCK1), Amrr::rate)
    }

    /// The TX counters since they were last read (they clear when read):
    /// frames sent counting their retries, and the retries and failures
    /// among them.
    fn tx_stats(&mut self) -> Result<(u32, u32)> {
        let fail = self.read(TX_STA_CNT0)? & 0xffff;
        let cnt1 = self.read(TX_STA_CNT1)?;
        self.read(TX_STA_CNT2)?;
        let retries = fail + (cnt1 >> 16);
        Ok((retries + (cnt1 & 0xffff), retries))
    }

    /// The network left: no BSSID, the access point's entry cleared.
    pub fn left(&mut self) -> Result<()> {
        if self.wcid != 0xff {
            self.set_region_4(WCID_ENTRY0 + 8 * u16::from(self.wcid), 0xffff_ffff, 2)?;
            self.wcid = 0xff;
        }
        self.amrr = None;
        self.set_bssid([0; 6])?;
        self.set_leds(LED_MODE_ON)
    }

    /// Send 802.11 frame `frame`, as it is (its duration aside).
    pub fn send(&mut self, frame: &[u8]) -> Result<()> {
        if frame.len() < 24 {
            return Err(Error::Usb);
        }
        let (queue, xfer) = tx::transfer(frame, self.tx_rate(), self.wcid, self.short_preamble);
        let ep = match queue {
            Queue::Mcu => self.eps.mcu,
            Queue::Data(ac) => self.eps.data[ac],
        };
        Ok(self.usb.bulk_out(ep, &xfer)?)
    }

    /// The endpoint the frames heard come in on.
    pub fn rx_endpoint(&self) -> u8 {
        self.eps.rx
    }

    /// Signal `rssi` (`rx::Frame`'s) in dBm (mtw_rssi2dbm).
    pub fn rssi_dbm(&self, rssi: u8) -> i8 {
        let delta = i32::from(self.ee.rssi_2ghz) - i32::from(self.ee.lna);
        (-12 - delta - i32::from(rssi)).clamp(-128, 0) as i8
    }

    /// Stop hearing and sending, the radio off (mtw_stop).
    pub fn stop(&mut self) -> Result<()> {
        let _ = self.set_leds(LED_MODE_ON);
        let tmp = self.read(WPDMA_GLO_CFG)?;
        self.write(WPDMA_GLO_CFG, tmp & !(RX_DMA_EN | TX_DMA_EN))?;
        self.write(USB_DMA_CFG, 0)?;
        for _ in 0..100 {
            if self.read(WPDMA_GLO_CFG)? & (TX_DMA_BUSY | RX_DMA_BUSY) == 0 {
                break;
            }
            self.usb.delay_us(10);
        }
        let tmp = self.read(MAC_SYS_CTRL)?;
        self.write(MAC_SYS_CTRL, tmp & !(MAC_RX_EN | MAC_TX_EN))?;
        let tmp = self.read(TX_RTS_CFG)?;
        self.write(TX_RTS_CFG, tmp & !0xff)?;
        let tmp = self.read(US_CYC_CNT)?;
        self.write(US_CYC_CNT, tmp & !0xff)?;
        let tmp = self.read(PBF_CFG)?;
        self.write(PBF_CFG, tmp & !0x3)?;
        for _ in 0..100 {
            if self.read(TXRXQ_PCNT)? & TX2Q_PCNT_MASK == 0 {
                break;
            }
        }
        self.usb.delay_us(1000);
        self.mcu_radio(0x30, 0x1)?;
        self.usb.delay_us(10_000);
        Ok(())
    }

    // ---- Registers -------------------------------------------------------

    pub fn read(&mut self, reg: u16) -> Result<u32> {
        read(&mut self.usb, reg)
    }

    pub fn write(&mut self, reg: u16, val: u32) -> Result<()> {
        self.usb.vendor_out(REQ_WRITE_2, val as u16, reg, &[])?;
        Ok(self.usb.vendor_out(REQ_WRITE_2, (val >> 16) as u16, reg + 2, &[])?)
    }

    fn read_cfg(&mut self, reg: u16) -> Result<u32> {
        let mut b = [0; 4];
        self.usb.vendor_in(REQ_READ_CFG, 0, reg, &mut b)?;
        Ok(u32::from_le_bytes(b))
    }

    fn write_cfg(&mut self, reg: u16, val: u32) -> Result<()> {
        Ok(self.usb.vendor_out(REQ_WRITE_CFG, 0, reg, &val.to_le_bytes())?)
    }

    fn write_region_1(&mut self, reg: u16, data: &[u8]) -> Result<()> {
        Ok(self.usb.vendor_out(REQ_WRITE_REGION_1, 0, reg, data)?)
    }

    fn set_region_4(&mut self, reg: u16, val: u32, count: u16) -> Result<()> {
        for i in 0..count {
            self.write(reg + 4 * i, val)?;
        }
        Ok(())
    }

    /// Wait until `bits` of `reg` are clear: `tries` reads `us` apart.
    fn wait(&mut self, reg: u16, bits: u32, tries: u32, us: u32, what: &'static str) -> Result<u32> {
        for _ in 0..tries {
            let v = self.read(reg)?;
            if v & bits == 0 {
                return Ok(v);
            }
            self.usb.delay_us(us);
        }
        Err(Error::Timeout(what))
    }

    fn wait_dma_idle(&mut self, tries: u32) -> Result<u32> {
        self.wait(WPDMA_GLO_CFG, TX_DMA_BUSY | RX_DMA_BUSY, tries, 1000, "the DMA engine")
    }

    /// A 16-bit word of the eFUSE ROM (mtw_efuse_read_2): its 16-byte
    /// block is read into EFUSE_DATA0 to 3.
    fn efuse_read_2(&mut self, addr: u16) -> Result<u16> {
        let addr = u32::from(addr) * 2;
        let mut tmp = self.read(EFUSE_CTRL)?;
        tmp &= !(EFSROM_MODE_MASK | EFSROM_AIN_MASK);
        tmp |= (addr & !0xf) << EFSROM_AIN_SHIFT | EFSROM_KICK;
        self.write(EFUSE_CTRL, tmp)?;
        let tmp = self.wait(EFUSE_CTRL, EFSROM_KICK, 100, 2, "the eFUSE ROM")?;
        if tmp & EFUSE_AOUT_MASK == EFUSE_AOUT_MASK {
            return Ok(0xffff);
        }
        let v = self.read(EFUSE_DATA0 + (addr & 0xc) as u16)?;
        Ok(if addr & 2 != 0 { (v >> 16) as u16 } else { v as u16 })
    }

    fn rf_read(&mut self, bank: u8, reg: u8) -> Result<u8> {
        self.wait(RF_CSR, RF_CSR_KICK, 100, 0, "the RF")?;
        let tmp = RF_CSR_KICK | u32::from(bank & 0xf) << BANK_SHIFT | u32::from(reg) << 8;
        self.write(RF_CSR, tmp)?;
        Ok(self.wait(RF_CSR, RF_CSR_KICK, 100, 0, "the RF")? as u8)
    }

    fn rf_write(&mut self, bank: u8, reg: u8, val: u8) -> Result<()> {
        self.wait(RF_CSR, RF_CSR_KICK, 10, 0, "the RF")?;
        let tmp = RF_CSR_WRITE | RF_CSR_KICK | u32::from(bank & 0xf) << BANK_SHIFT | u32::from(reg) << 8 | u32::from(val);
        self.write(RF_CSR, tmp)
    }

    fn bbp_read(&mut self, reg: u8) -> Result<u8> {
        self.wait(BBP_CSR, BBP_CSR_KICK, 10, 0, "the baseband")?;
        self.write(BBP_CSR, BBP_CSR_READ | BBP_CSR_KICK | u32::from(reg) << BBP_ADDR_SHIFT)?;
        Ok(self.wait(BBP_CSR, BBP_CSR_KICK, 10, 0, "the baseband")? as u8)
    }

    fn bbp_write(&mut self, reg: u8, val: u8) -> Result<()> {
        self.wait(BBP_CSR, BBP_CSR_KICK, 10, 0, "the baseband")?;
        self.write(BBP_CSR, BBP_CSR_KICK | u32::from(reg) << BBP_ADDR_SHIFT | u32::from(val))
    }

    // ---- The MCU ---------------------------------------------------------

    /// Command `cmd` with `data` to the MCU, on its endpoint
    /// (mtw_mcu_cmd; the sequence number stays 0, as there).
    fn mcu_cmd(&mut self, cmd: u8, data: &[u8]) -> Result<()> {
        let mut buf = alloc::vec::Vec::with_capacity(TXD_LEN + data.len() + DMA_PAD);
        buf.extend_from_slice(&(data.len() as u16).to_le_bytes());
        let flags = TXD_CMD | TXD_MCU | u16::from(cmd & 0x7f) << TXD_CMD_SHIFT;
        buf.extend_from_slice(&flags.to_le_bytes());
        buf.extend_from_slice(data);
        buf.extend_from_slice(&[0; DMA_PAD]);
        Ok(self.usb.bulk_out(self.eps.mcu, &buf)?)
    }

    fn mcu_calibrate(&mut self, func: u32, val: u32) -> Result<()> {
        let mut c = [0; 8];
        c[..4].copy_from_slice(&func.to_le_bytes());
        c[4..].copy_from_slice(&val.to_le_bytes());
        self.mcu_cmd(MCU_CMD_CALIBRATE, &c)
    }

    fn mcu_radio(&mut self, func: u32, val: u32) -> Result<()> {
        let mut c = [0; 16];
        c[..4].copy_from_slice(&func.to_le_bytes());
        c[4..8].copy_from_slice(&val.to_le_bytes());
        self.mcu_cmd(MCU_CMD_RADIO, &c)
    }

    fn set_leds(&mut self, mode: u32) -> Result<()> {
        let mut c = [0; 8];
        c[..4].copy_from_slice(&1u32.to_le_bytes());
        c[4..].copy_from_slice(&mode.to_le_bytes());
        self.mcu_cmd(MCU_CMD_LED, &c)
    }

    /// The MCU's DMA set up for the firmware (mtw_ucode_setup).
    fn ucode_setup(&mut self) -> Result<()> {
        self.write(USB_DMA_CFG, USB_TX_EN | USB_RX_EN)?;
        self.write(FCE_PSE_CTRL, 1)?;
        self.write(TX_CPU_FCE_BASE, 0x0040_0230)?;
        self.write(TX_CPU_FCE_MAX_COUNT, 1)?;
        self.write(MCU_FW_IDX, 1)?;
        self.write(FCE_PDMA, 0x44)?;
        self.write(FCE_SKIP_FS, 3)
    }

    /// `data` into the MCU's memory at `offset`, a block per bulk transfer
    /// after its address and length (mtw_ucode_write).
    fn ucode_write(&mut self, data: &[u8], offset: u32) -> Result<()> {
        let mut buf = alloc::vec::Vec::with_capacity(TXD_LEN + FW_BLOCK + DMA_PAD);
        let mut sent = 0;
        for block in data.chunks(FW_BLOCK) {
            buf.clear();
            buf.extend_from_slice(&(block.len() as u16).to_le_bytes());
            buf.extend_from_slice(&(TXD_DATA | TXD_MCU).to_le_bytes());
            buf.extend_from_slice(block);
            buf.extend_from_slice(&[0; DMA_PAD]);
            self.write_cfg(MCU_DMA_ADDR, offset + sent)?;
            self.write_cfg(MCU_DMA_LEN, (block.len() as u32) << 16)?;
            self.usb.bulk_out(self.eps.mcu, &buf)?;
            // Written back as it was read, as mtw_ucode_write does.
            let idx = self.read(MCU_FW_IDX)?;
            self.write(MCU_FW_IDX, idx)?;
            sent += block.len() as u32;
        }
        Ok(())
    }

    /// Load and start the firmware, unless it runs already
    /// (mtw_load_microcode): whether it was loaded.
    fn load_microcode(&mut self, fw: &Firmware) -> Result<bool> {
        if self.read_cfg(MCU_DMA_ADDR)? == MCU_READY {
            return Ok(false);
        }
        self.write(FCE_PDMA, 0)?;
        self.write(FCE_PSE_CTRL, 0)?;
        self.ucode_setup()?;
        self.ucode_write(fw.ilm, self.chip.ilm_offset)?;
        if !fw.dlm.is_empty() && self.chip.dlm_offset > 0 {
            self.ucode_write(fw.dlm, self.chip.dlm_offset)?;
        }
        // The interrupt vectors start it.
        self.usb.vendor_out(REQ_RESET, 0x12, 0, fw.ivb)?;
        self.usb.delay_us(10_000);
        for _ in 0..100 {
            if self.read_cfg(MCU_DMA_ADDR)? & MCU_READY != 0 {
                return Ok(true);
            }
            self.usb.delay_us(100_000);
        }
        Err(Error::Timeout("the MCU to start"))
    }

    // ---- Bring-up --------------------------------------------------------

    /// The WLAN core's power and clock, the oscillator (mtw_wlan_enable).
    fn wlan_enable(&mut self) -> Result<()> {
        let mut tmp = self.read(WLAN_CTRL)?;
        tmp &= !WLAN_CLK_EN;
        tmp |= WLAN_EN;
        self.write(WLAN_CTRL, tmp)?;
        self.usb.delay_us(2000);
        tmp |= WLAN_CLK_EN;
        self.write(WLAN_CTRL, tmp)?;
        self.usb.delay_us(2000);
        let mut tmp = self.read(OSC_CTRL)?;
        tmp |= OSC_EN;
        self.write(OSC_CTRL, tmp)?;
        tmp |= OSC_CAL_REQ;
        self.write(OSC_CTRL, tmp)
    }

    /// The eFUSE ROM: the RF, antennas, address, calibration
    /// (mtw_read_eeprom, mtw_get_txpower).
    fn read_eeprom(&mut self) -> Result<Info> {
        let rf_rev = self.efuse_read_2(EEPROM_CHIPID)?;
        let ant = self.efuse_read_2(EEPROM_ANTENNA)?;
        let mut mac = [0; 6];
        for (i, w) in [EEPROM_MAC01, EEPROM_MAC23, EEPROM_MAC45].into_iter().enumerate() {
            mac[2 * i..2 * i + 2].copy_from_slice(&self.efuse_read_2(w)?.to_le_bytes());
        }
        let v = self.efuse_read_2(EEPROM_FREQ_OFFSET)?;
        self.ee.freq_offset = if v & 0xff != 0xff { v as u8 } else { 0 };
        for i in (0..14).step_by(2) {
            let v = self.efuse_read_2(EEPROM_PWR2GHZ_BASE1 + i as u16 / 2)?;
            self.ee.txpow1[i] = v as u8 as i8;
            self.ee.txpow1[i + 1] = (v >> 8) as u8 as i8;
        }
        // Broken entries.
        for p in &mut self.ee.txpow1 {
            if !(0..=27).contains(p) {
                *p = 5;
            }
        }
        for ridx in 0..5u16 {
            let lo = self.efuse_read_2(EEPROM_RPWR + ridx * 2)?;
            let hi = self.efuse_read_2(EEPROM_RPWR + ridx * 2 + 1)?;
            self.ee.txpow20mhz[usize::from(ridx)] = u32::from(lo) | u32::from(hi) << 16;
        }
        let rssi = self.efuse_read_2(EEPROM_RSSI1_2GHZ)? as u8 as i8;
        self.ee.rssi_2ghz = if (-10..=10).contains(&rssi) { rssi } else { 0 };
        self.ee.lna = self.efuse_read_2(EEPROM_LNA)? as u8;
        Ok(Info {
            mac,
            mac_ver: 0,
            mac_rev: 0,
            rf_rev,
            ntx: (ant >> 4) as u8 & 0xf,
            nrx: ant as u8 & 0xf,
            fw: None,
        })
    }

    fn bbp_init(&mut self) -> Result<()> {
        // Until the baseband wakes up.
        let mut awake = false;
        for _ in 0..20 {
            let b = self.bbp_read(0)?;
            if b != 0 && b != 0xff {
                awake = true;
                break;
            }
        }
        if !awake {
            return Err(Error::Timeout("the baseband to wake up"));
        }
        self.bbp_read(3)?;
        self.bbp_write(3, 0)?;
        self.bbp_read(105)?;
        self.bbp_write(105, 0)?;
        for &(reg, val) in &DEF_BBP {
            self.bbp_write(reg, val)?;
        }
        self.chan_group = None;
        Ok(())
    }

    fn rf_init(&mut self) -> Result<()> {
        for (bank, table) in [(0, &RF_BANK0[..]), (4, &RF_BANK4[..]), (5, &RF_BANK5[..])] {
            for &(reg, val) in table {
                self.rf_write(bank, reg, val)?;
            }
        }
        Ok(())
    }

    /// RF registers, frequency offset, the MCU's calibrations
    /// (mt7601_rf_setup), once.
    fn rf_setup(&mut self) -> Result<()> {
        if self.rf_calibrated {
            return Ok(());
        }
        self.rf_init()?;
        self.rf_write(0, 12, self.ee.freq_offset)?;
        self.rf_read(0, 12)?;
        self.rf_temperature()?;
        self.rf_read(0, 7)?;
        self.mcu_calibrate(0x1, 0)?;
        self.usb.delay_us(100_000);
        self.rf_read(0, 7)?;

        // VCO calibration, RF 0/4.
        self.rf_write(0, 4, 0x0a)?;
        self.rf_write(0, 4, 0x20)?;
        let rf = self.rf_read(0, 4)?;
        self.rf_write(0, 4, rf | 0x80)?;

        self.mcu_calibrate(0x9, 0)?;
        self.rxdc_cal()?;
        self.mcu_calibrate(0x6, 1)?;
        self.mcu_calibrate(0x6, 0)?;
        self.mcu_calibrate(0x4, 0)?;
        self.mcu_calibrate(0x5, 0)?;
        let tmp = self.read(LDO_CFG0)?;
        self.write(LDO_CFG0, (tmp & !(1 << 4)) | 1 << 2)?;
        self.mcu_calibrate(0x8, 0)?;
        self.rxdc_cal()?;
        self.rf_calibrated = true;
        Ok(())
    }

    /// The chip's temperature, read through the baseband
    /// (mt7601_rf_temperature: its value is not kept).
    fn rf_temperature(&mut self) -> Result<i8> {
        let rfb = self.read(RF_BYPASS0)?;
        let rfs = self.read(RF_SETTING0)?;
        self.write(RF_BYPASS0, 0)?;
        self.write(RF_SETTING0, 0x10)?;
        self.write(RF_BYPASS0, 0x10)?;
        let b = self.bbp_read(47)?;
        self.bbp_write(47, (b & !0x7f) | 0x10)?;
        self.bbp_write(22, 0x40)?;
        let mut done = false;
        for _ in 0..10 {
            if self.bbp_read(47)? & 0x10 == 0 {
                done = true;
                break;
            }
        }
        if !done {
            return Err(Error::Timeout("the temperature"));
        }
        // mt7601_r49_read.
        self.bbp_read(47)?;
        self.bbp_write(47, 0x90)?;
        self.bbp_write(47, 0x90 | R47_TEMP)?;
        let t = self.bbp_read(49)? as i8;
        self.bbp_write(22, 0)?;
        let b = self.bbp_read(21)?;
        self.bbp_write(21, b | 0x02)?;
        self.bbp_write(21, b & !0x02)?;
        self.write(RF_BYPASS0, 0)?;
        self.write(RF_SETTING0, rfs)?;
        self.write(RF_BYPASS0, rfb)?;
        Ok(t)
    }

    /// Receive DC calibration (mt7601_rxdc_cal).
    fn rxdc_cal(&mut self) -> Result<()> {
        let saved = self.read(MAC_SYS_CTRL)?;
        self.write(MAC_SYS_CTRL, MAC_RX_EN)?;
        self.bbp_write(158, 0x8d)?;
        self.bbp_write(159, 0xfc)?;
        self.bbp_write(158, 0x8c)?;
        self.bbp_write(159, 0x4c)?;
        let mut done = false;
        for _ in 0..20 {
            self.usb.delay_us(300);
            self.bbp_write(158, 0x8c)?;
            if self.bbp_read(159)? == 0x0c {
                done = true;
                break;
            }
        }
        if !done {
            return Err(Error::Timeout("the RX DC calibration"));
        }
        self.write(MAC_SYS_CTRL, 0)?;
        self.bbp_write(158, 0x8d)?;
        self.bbp_write(159, 0xe0)?;
        self.write(MAC_SYS_CTRL, saved)
    }

    /// 2.4 GHz at 20 MHz, its baseband calibration and AGC
    /// (mtw_select_chan_group, group 0).
    fn select_chan_group(&mut self, group: u8) -> Result<()> {
        let tmp = self.read(TX_BAND_CFG)?;
        let band = if group == 0 { TX_BAND_SEL_2G } else { TX_BAND_SEL_5G };
        self.write(TX_BAND_CFG, (tmp & !(TX_BAND_SEL_2G | TX_BAND_SEL_5G | TX_BAND_UPPER_40M)) | band)?;
        self.select_20mhz()?;
        for (reg, val) in [
            (69, 0x12),
            (91, 0x07),
            (195, 0x23),
            (196, 0x17),
            (195, 0x24),
            (196, 0x06),
            (195, 0x81),
            (196, 0x12),
            (195, 0x83),
            (196, 0x17),
        ] {
            self.bbp_write(reg, val)?;
        }
        self.rf_write(5, 8, 0x00)?;
        self.mcu_calibrate(0x6, 0x10001)?;
        // The initial AGC (mt7601_set_agc).
        let agc = 0x14;
        self.bbp_write(66, agc)?;
        self.bbp_write(195, 0x87)?;
        self.bbp_write(196, (agc & 0xf0) | 0x08)
    }

    fn select_20mhz(&mut self) -> Result<()> {
        let b = self.bbp_read(4)?;
        self.bbp_write(4, (b & !0x18) | 0x40)
    }

    /// The DMA engines on, receive aggregation, the receive filter of a
    /// station, then the MAC (mtw_txrx_enable).
    fn txrx_enable(&mut self) -> Result<()> {
        self.write(MAC_SYS_CTRL, MAC_TX_EN)?;
        let mut tmp = self.wait_dma_idle(200)?;
        self.usb.delay_us(50);
        tmp |= RX_DMA_EN | TX_DMA_EN | TX_WB_DDONE;
        self.write(WPDMA_GLO_CFG, tmp)?;
        // Aggregated transfers of up to 2 KiB of frames: a page holds one.
        let agg = USB_TX_EN | USB_RX_EN | USB_RX_AGG_EN | usb_rx_agg_to(128) | usb_rx_agg_lmt(2);
        self.write(USB_DMA_CFG, agg)?;
        let filter = DROP_CRC_ERR
            | DROP_PHY_ERR
            | DROP_UC_NOME
            | DROP_DUPL
            | DROP_CTS
            | DROP_BA
            | DROP_ACK
            | DROP_VER_ERR
            | DROP_CTRL_RSV
            | DROP_CFACK
            | DROP_CFEND
            | DROP_RTS
            | DROP_PSPOLL;
        self.write(RX_FILTR_CFG, filter)?;
        self.write(MAC_SYS_CTRL, MAC_RX_EN | MAC_TX_EN)
    }

    fn set_macaddr(&mut self, a: [u8; 6]) -> Result<()> {
        self.write(MAC_ADDR_DW0, u32::from_le_bytes([a[0], a[1], a[2], a[3]]))?;
        self.write(MAC_ADDR_DW1, u32::from(a[4]) | u32::from(a[5]) << 8 | 0xff << 16)
    }

    fn set_bssid(&mut self, b: [u8; 6]) -> Result<()> {
        self.write(MAC_BSSID_DW0, u32::from_le_bytes([b[0], b[1], b[2], b[3]]))?;
        self.write(MAC_BSSID_DW1, u32::from(b[4]) | u32::from(b[5]) << 8)
    }

    fn set_txrts(&mut self) -> Result<()> {
        let tmp = self.read(TX_RTS_CFG)?;
        self.write(TX_RTS_CFG, (tmp & !0x00ff_ff00) | 0x1000 << RTS_THRES_SHIFT)
    }

    fn abort_tsf_sync(&mut self) -> Result<()> {
        let tmp = self.read(BCN_TIME_CFG)?;
        self.write(BCN_TIME_CFG, tmp & !(BCN_TX_EN | TSF_TIMER_EN | TBTT_TIMER_EN))
    }

    /// The hardware's rate fallback on retries: OFDM down to 6 Mb/s, CCK
    /// down to 1 (mtw_enable_mrr).
    fn enable_mrr(&mut self) -> Result<()> {
        let ofdm = |m: u32| 1 << 3 | m;
        self.write(
            LG_FBK_CFG0,
            ofdm(6) << 28
                | ofdm(5) << 24
                | ofdm(4) << 20
                | ofdm(3) << 16
                | ofdm(2) << 12
                | ofdm(1) << 8
                | ofdm(0) << 4
                | ofdm(0),
        )?;
        self.write(LG_FBK_CFG1, 2 << 12 | 1 << 8)
    }
}

/// LEGACY_BASIC_RATE: a bit per `reg::RATES` index the access point says
/// is basic (mtw_set_basicrates' 0x17f for 802.11g when it says none).
fn basic_rates(rates: &[u8]) -> u32 {
    let mask = (0..RATES.len())
        .filter(|&r| rates.iter().any(|&v| v & 0x80 != 0 && v & 0x7f == RATES[r].rate))
        .fold(0, |m, r| m | 1 << r);
    if mask == 0 { 0x17f } else { mask }
}

/// A register: four bytes of the device's (mtw_read).
fn read<U: Usb>(usb: &mut U, reg: u16) -> Result<u32> {
    let mut b = [0; 4];
    usb.vendor_in(REQ_READ_REGION_1, 0, reg, &mut b)?;
    Ok(u32::from_le_bytes(b))
}
