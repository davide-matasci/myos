//! The MT76x2U: its registers through vendor requests, the EEPROM, the
//! ROM patch and firmware, the MCU's commands and answers, the MAC and
//! baseband bring-up, the channels and their calibration, joining and
//! sending (Linux's mt76x2u_init_hardware, mt76x2u_mcu_*, mt76x2u_mac_*,
//! mt76x2u_phy_set_channel, mt76x2u_phy_calibrate and the mt76x02 and
//! mt76x2 helpers they call; notices in LICENSE). At 20 MHz: no 40 or
//! 80 MHz channels, no VHT (issue #563).

use alloc::vec::Vec;

use super::frame::{self, Tx};
use super::fw::{Firmware, RomPatch};
use super::reg::*;
use crate::chip::Chip;
use crate::dev::{Error, Info, Peer, Result};
use crate::rate::{Amrr, RateSet, TxRate};
use crate::usb::{Endpoints, Usb};

/// The 2.4 GHz channels, and the 5 GHz ones (the radar channels, 52 to
/// 144, are heard and not probed, as a station must), at 20 MHz.
pub const CHANNELS_2G: [u8; 13] = [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13];
pub const CHANNELS_5G: [u8; 25] = [
    36, 40, 44, 48, 52, 56, 60, 64, 100, 104, 108, 112, 116, 120, 124, 128, 132, 136, 140, 144, 149, 153, 157, 161,
    165,
];

/// Whether 5 GHz channel `chan` is one where radar has precedence (DFS):
/// the radio sends nothing there before it hears an access point.
pub fn radar(chan: u8) -> bool {
    (52..=144).contains(&chan)
}

/// The chain masks (two RX, two TX: mphy.chainmask 0x202).
const CHAINMASK: u16 = 0x202;
/// The transmit power limit per chain, in 0.5 dB: 20 dBm, less 3 dB for
/// two chains (txpower_conf as mac80211's power level makes it).
const TXPOWER_CONF: i32 = 2 * 20 - 6;
/// The station's interface: AP-client bss index 8 (mt76x02_add_interface).
const VIF_IDX: u8 = 8;
/// The access point's entry in the station table.
const WCID_AP: u8 = 1;

/// The calibration state (struct mt76x02_calibration).
#[derive(Default)]
struct Cal {
    /// The R and RC calibrations, done once.
    init_cal_done: bool,
    channel_cal_done: bool,
    tssi_cal_done: bool,
    tssi_comp_pending: bool,
    dpd_cal_done: bool,
    gain_init_done: bool,
    high_gain: [i8; 2],
    rssi_offset: [i8; 2],
    lna_gain: i8,
    mcu_gain: u32,
    agc_gain_init: [u8; 2],
    agc_gain_cur: [u8; 2],
    agc_gain_adjust: u8,
    low_gain: i8,
}

/// The transmit power of each rate (struct mt76x02_rate_power, its `all`).
#[derive(Default, Clone, Copy)]
struct RatePower {
    cck: [i8; 4],
    ofdm: [i8; 8],
    ht: [i8; 16],
    vht: [i8; 2],
}

impl RatePower {
    fn all(&mut self) -> impl Iterator<Item = &mut i8> {
        self.cck.iter_mut().chain(self.ofdm.iter_mut()).chain(self.ht.iter_mut()).chain(self.vht.iter_mut())
    }
}

/// One chain's power calibration for a channel, and the target
/// (struct mt76x2_tx_power_info).
#[derive(Default)]
struct PowerInfo {
    target_power: u8,
    chain: [ChainPower; 2],
}

#[derive(Default, Clone, Copy)]
struct ChainPower {
    tssi_slope: u8,
    tssi_offset: u8,
    target_power: i8,
    delta: i8,
}

pub struct Dev<U: Usb> {
    pub usb: U,
    pub chip: &'static Chip,
    eps: Endpoints,
    /// ASIC_VERSION: the chip (its high half) and revision.
    asic: u32,
    ee: [u8; EEPROM_SIZE],
    /// The last MCU message's sequence number.
    seq: u8,
    rxfilter: u32,
    chan: u8,
    cal: Cal,
    /// The access point's table entry while associated (0xff: none), the
    /// rates frames to it go at, its preamble, its signal averaged.
    wcid: u8,
    amrr: Option<Amrr>,
    short_preamble: bool,
    avg_rssi: Option<i32>,
}

impl<U: Usb> Dev<U> {
    /// The radio `chip` behind `usb` with bulk endpoints `eps`, once its
    /// ASIC answers and is one of the chip's (mt76x2u_probe).
    pub fn new(mut usb: U, chip: &'static Chip, eps: Endpoints) -> Result<Self> {
        let mut asic = 0;
        for _ in 0..100 {
            asic = read(&mut usb, ASIC_VERSION)?;
            if asic != 0 && asic != 0xffff_ffff {
                break;
            }
            usb.delay_us(10);
        }
        if asic == 0 || asic == 0xffff_ffff {
            return Err(Error::Timeout("the NIC to initialize"));
        }
        if !chip.asic.contains(&((asic >> 16) as u16)) {
            return Err(Error::Chip((asic >> 16) as u16));
        }
        Ok(Dev {
            usb,
            chip,
            eps,
            asic,
            ee: [0; EEPROM_SIZE],
            seq: 0,
            rxfilter: 0,
            chan: 0,
            cal: Cal::default(),
            wcid: 0xff,
            amrr: None,
            short_preamble: false,
            avg_rssi: None,
        })
    }

    fn rev(&self) -> u16 {
        self.asic as u16
    }

    /// Read the EEPROM, power the WLAN core, load the ROM patch and
    /// firmware, bring the MAC up and stop it again until `init`
    /// (mt76x2u_init_eeprom, mt76x2u_init_hardware).
    pub fn attach(&mut self, patch: &RomPatch, fw: &Firmware) -> Result<Info> {
        for at in (0..EEPROM_SIZE).step_by(4) {
            let v = self.read(TYPE_EEPROM | at as u32)?;
            self.ee[at..at + 4].copy_from_slice(&v.to_le_bytes());
        }
        self.reset_wlan()?;
        self.power_on()?;
        self.wait_for_mac()?;
        self.load_rom_patch(patch)?;
        self.load_firmware(fw)?;
        self.poll(WPDMA_GLO_CFG, TX_DMA_BUSY | RX_DMA_BUSY, 0, 10, 10_000, "the DMA engine")?;
        // Until the ASIC is ready after the firmware.
        self.wait_for_mac()?;
        self.init_dma()?;
        self.mcu_function_select(Q_SELECT, 1)?;
        self.mcu_radio(true)?;
        self.mac_reset()?;
        let mac = self.ee_mac();
        self.set_macaddr(mac)?;
        self.rxfilter = self.read(RX_FILTR_CFG)?;
        self.poll(MAC_STATUS, STATUS_TX | STATUS_RX, 0, 10, 10_000, "the MAC to be idle")?;
        // The station table and the shared keys cleared: the daemon keeps
        // the keys.
        for i in 0..=255 {
            self.wcid_setup(i, 0, None)?;
        }
        for vif in 0..16 {
            for key in 0..4 {
                self.shared_key_clear(vif, key)?;
            }
        }
        self.init_beacon_config()?;
        self.rmw(US_CYC_CFG, US_CYC_CNT, 0x1e)?;
        self.write(TXOP_CTRL_CFG, 0x583f)?;
        self.mcu_load_cr(RF_BBP_CR, 0, 0)?;
        self.set_rxpath()?;
        self.set_txdac()?;
        self.mac_stop_usb()?;
        Ok(Info {
            mac,
            mac_ver: (self.asic >> 16) as u16,
            mac_rev: self.rev(),
            rf_rev: self.ee16(EE_CHIP_ID),
            ntx: 2,
            nrx: 2,
            fw: Some((fw.build_ver, fw.fw_ver)),
        })
    }

    /// Start hearing and sending as station `mac`, on the first channel
    /// it has (mt76x2u_start, the receive filter of a station as
    /// mt76x02_configure_filter and mt76x2u_config make it).
    pub fn init(&mut self, mac: [u8; 6]) -> Result<()> {
        if mac != self.ee_mac() {
            self.set_macaddr(mac)?;
        }
        self.rxfilter &= !FILTR_OTHER_BSS;
        self.rxfilter |= FILTR_CRC_ERR | FILTR_PHY_ERR | FILTR_CONTROL | FILTR_PSPOLL | FILTR_PROMISC;
        self.mac_start()?;
        let first = self.channels()[0];
        self.tune(first, false)
    }

    /// The channels the board has (its EEPROM's board type: 2.4 GHz, 5,
    /// or both; mt76x02_eeprom_parse_hw_cap).
    pub fn channels(&self) -> Vec<u8> {
        let board = (self.ee16(EE_NIC_CONF_0) >> NIC_CONF_0_BOARD_TYPE_SHIFT) & 3;
        let mut c = Vec::new();
        if board != BOARD_TYPE_5GHZ {
            c.extend_from_slice(&CHANNELS_2G);
        }
        if board != BOARD_TYPE_2GHZ {
            c.extend_from_slice(&CHANNELS_5G);
        }
        c
    }

    /// Hear and send on channel `chan` for a scan: tuned, its gains and
    /// power set, not calibrated (mac80211's scanning state).
    pub fn set_chan(&mut self, chan: u8) -> Result<()> {
        self.tune(chan, true)
    }

    /// About to authenticate with `peer`: on its channel, calibrated, its
    /// address the MAC's BSSID.
    pub fn joining(&mut self, peer: &Peer) -> Result<()> {
        self.tune(peer.channel, false)?;
        self.set_bssid(peer.bssid)
    }

    /// Associated to `peer` as `aid`, over 802.11n when `ht` (the
    /// association asked for it): the slot time and ACK timeout, the
    /// preamble, the access point in the station table, its rates for
    /// AMRR over the two streams, which starts at the slowest
    /// (mt76x02_bss_info_changed, mt76x02_sta_add). The aid stays the
    /// daemon's: the MAC does not need it.
    pub fn associated(&mut self, peer: &Peer, _aid: u16, ht: bool) -> Result<()> {
        self.set_tx_ackto(if peer.short_slot { 9 } else { 20 })?;
        if peer.short_preamble {
            self.set(AUTO_RSP_CFG, AUTO_RSP_PREAMB_SHORT)?;
        } else {
            self.clear(AUTO_RSP_CFG, AUTO_RSP_PREAMB_SHORT)?;
        }
        self.set_bssid(peer.bssid)?;
        self.wcid_setup(WCID_AP, VIF_IDX, Some(peer.bssid))?;
        self.wcid = WCID_AP;
        self.short_preamble = peer.short_preamble;
        self.amrr = Some(Amrr::new(RateSet::new(&peer.rates, peer.ht_mcs.filter(|_| ht), 2)));
        self.avg_rssi = None;
        // The counters start from the association.
        self.tx_stats()?;
        Ok(())
    }

    /// Once a second: the last second's TX statistics to AMRR, which may
    /// change the rate (as `dev`'s), then the radio's own periodic work:
    /// the channel's calibration if a scan left it undone, the transmit
    /// power's compensation, the receive gain for the signal of the
    /// access point (mt76x2u_phy_calibrate).
    pub fn calibrate(&mut self) -> Result<()> {
        if self.amrr.is_some() {
            let (tx, retries) = self.tx_stats()?;
            if let Some(a) = self.amrr.as_mut() {
                a.update(tx, retries);
            }
        }
        if self.chan == 0 {
            return Ok(());
        }
        self.channel_calibrate(false)?;
        self.tssi_compensate()?;
        self.update_channel_gain()
    }

    /// The rate data frames to the access point go at now (the band's
    /// slowest when not associated).
    pub fn tx_rate(&self) -> TxRate {
        let slowest = TxRate::Legacy(if self.chan > 14 { 4 } else { crate::reg::RIDX_CCK1 });
        self.amrr.as_ref().map_or(slowest, Amrr::rate)
    }

    /// The TX counters since they were last read (they clear when read):
    /// frames sent counting their retries, and the retries and failures
    /// among them.
    fn tx_stats(&mut self) -> Result<(u32, u32)> {
        let fail = self.read(TX_STA_0)? & 0xffff;
        let cnt1 = self.read(TX_STA_1)?;
        self.read(TX_STA_2)?;
        let retries = fail + (cnt1 >> 16);
        Ok((retries + (cnt1 & 0xffff), retries))
    }

    /// The network left: the access point's entry cleared, no BSSID.
    pub fn left(&mut self) -> Result<()> {
        if self.wcid != 0xff {
            self.wcid_setup(self.wcid, 0, None)?;
            self.wcid = 0xff;
        }
        self.amrr = None;
        self.set_bssid([0; 6])
    }

    /// Send 802.11 frame `frame`, as it is (its duration aside).
    pub fn send(&mut self, frame: &[u8]) -> Result<()> {
        if frame.len() < 24 {
            return Err(Error::Usb);
        }
        let tx = Tx {
            rate: self.tx_rate(),
            wcid: self.wcid,
            short_preamble: self.short_preamble,
            ghz5: self.chan > 14,
            rev: self.rev(),
        };
        let (ac, xfer) = frame::transfer(frame, &tx);
        Ok(self.usb.bulk_out(self.eps.data[ac], &xfer)?)
    }

    /// The endpoint the frames heard come in on.
    pub fn rx_endpoint(&self) -> u8 {
        self.eps.rx
    }

    /// Each good frame of a transfer from the frames' endpoint to `f`,
    /// with its signal in dBm; the access point's signal is averaged for
    /// the receive gain.
    pub fn frames(&mut self, xfer: &mut [u8], mut f: impl FnMut(&[u8], i8)) {
        let mut ap = Vec::new();
        frame::frames(xfer, |fr| {
            let dbm = self.rssi_dbm(fr.rssi);
            if self.wcid != 0xff && fr.wcid == self.wcid {
                ap.push(i32::from(dbm));
            }
            f(fr.frame, dbm);
        });
        for s in ap {
            // mac80211's EWMA of the signal, weight 1/8.
            self.avg_rssi = Some(self.avg_rssi.map_or(s, |a| a - a / 8 + s / 8));
        }
    }

    /// Each chain's signal (RXWI rssi) in dBm, combined as two antennas
    /// add up (mt76x02_mac_get_rssi, mt76_rx_signal).
    pub fn rssi_dbm(&self, rssi: [i8; 2]) -> i8 {
        let mut signal = -128;
        for (chain, &r) in rssi.iter().enumerate() {
            let mut cur = (r as i32 + i32::from(self.cal.rssi_offset[chain])) as i8 as i32;
            cur = (cur - i32::from(self.cal.lna_gain)) as i8 as i32;
            if cur > 0 {
                continue;
            }
            if cur > signal {
                core::mem::swap(&mut cur, &mut signal);
            }
            signal += match signal - cur {
                0 => 3,
                1..=2 => 2,
                3..=6 => 1,
                _ => 0,
            };
        }
        signal.clamp(-128, 0) as i8
    }

    /// Stop hearing and sending, the radio off (mt76x2u_cleanup).
    pub fn stop(&mut self) -> Result<()> {
        self.mcu_radio(false)?;
        self.mac_stop_usb()
    }

    // ---- Registers -------------------------------------------------------

    /// Register `addr`: the MAC's, or the configuration's or EEPROM's
    /// (TYPE_CFG, TYPE_EEPROM) (__mt76u_rr).
    pub fn read(&mut self, addr: u32) -> Result<u32> {
        read(&mut self.usb, addr)
    }

    pub fn write(&mut self, addr: u32, val: u32) -> Result<()> {
        let req = if addr & TYPE_MASK == TYPE_CFG { REQ_WRITE_CFG } else { REQ_MULTI_WRITE };
        let a = addr & !TYPE_MASK;
        Ok(self.usb.vendor_out(req, (a >> 16) as u16, a as u16, &val.to_le_bytes())?)
    }

    fn rmw(&mut self, addr: u32, mask: u32, val: u32) -> Result<()> {
        let v = self.read(addr)?;
        self.write(addr, (v & !mask) | val)
    }

    fn set(&mut self, addr: u32, bits: u32) -> Result<()> {
        self.rmw(addr, 0, bits)
    }

    fn clear(&mut self, addr: u32, bits: u32) -> Result<()> {
        self.rmw(addr, bits, 0)
    }

    /// Field `mask` of `addr` set to `val` (mt76_rmw_field).
    fn rmw_field(&mut self, addr: u32, mask: u32, val: u32) -> Result<()> {
        self.rmw(addr, mask, (val << mask.trailing_zeros()) & mask)
    }

    fn get_field(&mut self, addr: u32, mask: u32) -> Result<u32> {
        Ok((self.read(addr)? & mask) >> mask.trailing_zeros())
    }

    /// Bytes from `addr` on (mt76u_copy).
    fn write_copy(&mut self, addr: u32, data: &[u8]) -> Result<()> {
        Ok(self.usb.vendor_out(REQ_MULTI_WRITE, 0, addr as u16, data)?)
    }

    /// Wait until `reg` & `mask` is `val`: `tries` reads `us` apart.
    fn poll(&mut self, reg: u32, mask: u32, val: u32, tries: u32, us: u32, what: &'static str) -> Result<()> {
        for _ in 0..tries {
            if self.read(reg)? & mask == val {
                return Ok(());
            }
            self.usb.delay_us(us);
        }
        Err(Error::Timeout(what))
    }

    fn ee16(&self, at: usize) -> u16 {
        u16::from_le_bytes([self.ee[at], self.ee[at + 1]])
    }

    fn ee_mac(&self) -> [u8; 6] {
        self.ee[EE_MAC_ADDR..EE_MAC_ADDR + 6].try_into().unwrap()
    }

    // ---- The MCU ---------------------------------------------------------

    /// Message `cmd` with `data` to the MCU on its endpoint, and when
    /// `wait` its answer on the other IN endpoint
    /// (__mt76x02u_mcu_send_msg, mt76x02u_mcu_wait_resp).
    fn mcu_send(&mut self, cmd: u8, data: &[u8], wait: bool) -> Result<()> {
        let mut seq = 0;
        if wait {
            self.seq = (self.seq + 1) & 0xf;
            if self.seq == 0 {
                self.seq = 1;
            }
            seq = self.seq;
        }
        let len = (data.len() + 3) & !3;
        let info = len as u32
            | CPU_TX_PORT << MSG_PORT_SHIFT
            | MSG_TYPE_CMD
            | u32::from(cmd) << MSG_CMD_SHIFT
            | u32::from(seq) << MSG_SEQ_SHIFT;
        let mut buf = Vec::with_capacity(4 + len + 4);
        buf.extend_from_slice(&info.to_le_bytes());
        buf.extend_from_slice(data);
        buf.resize(4 + len + 4, 0);
        self.usb.bulk_out(self.eps.mcu, &buf)?;
        if !wait {
            return Ok(());
        }
        let mut resp = [0u8; MCU_RESP_SIZE];
        for _ in 0..5 {
            let n = self.usb.bulk_in(self.eps.resp, &mut resp, 300)?;
            if n < 4 {
                continue;
            }
            let rxfce = u32::from_le_bytes(resp[..4].try_into().unwrap());
            if (rxfce >> RX_FCE_SEQ_SHIFT) & 0xf == u32::from(seq) && (rxfce >> RX_FCE_EVT_SHIFT) & 0xf == EVT_CMD_DONE {
                return Ok(());
            }
        }
        Err(Error::Timeout("the MCU to answer"))
    }

    /// Two 32-bit words to the MCU.
    fn mcu_send2(&mut self, cmd: u8, a: u32, b: u32, wait: bool) -> Result<()> {
        let mut m = [0; 8];
        m[..4].copy_from_slice(&a.to_le_bytes());
        m[4..].copy_from_slice(&b.to_le_bytes());
        self.mcu_send(cmd, &m, wait)
    }

    /// FUN_SET: Q_SELECT is not answered (mt76x02_mcu_function_select).
    fn mcu_function_select(&mut self, func: u32, val: u32) -> Result<()> {
        self.mcu_send2(CMD_FUN_SET_OP, func, val, func != Q_SELECT)
    }

    fn mcu_radio(&mut self, on: bool) -> Result<()> {
        self.mcu_send2(CMD_POWER_SAVING_OP, if on { RADIO_ON } else { RADIO_OFF }, 0, false)
    }

    fn mcu_calibrate(&mut self, kind: u32, param: u32) -> Result<()> {
        self.mcu_send2(CMD_CALIBRATION_OP, kind, param, true)
    }

    /// The RF and baseband settings the MCU keeps, with the EEPROM's NIC
    /// configuration (mt76x2_mcu_load_cr).
    fn mcu_load_cr(&mut self, mode: u8, temp: u8, chan: u8) -> Result<()> {
        let cfg = 1 << 31 | (u32::from(self.ee16(EE_NIC_CONF_0)) >> 8) & 0xff | (u32::from(self.ee16(EE_NIC_CONF_1)) << 8) & 0xff00;
        let mut m = [mode, temp, chan, 0, 0, 0, 0, 0];
        m[4..].copy_from_slice(&cfg.to_le_bytes());
        self.mcu_send(CMD_LOAD_CR, &m, true)
    }

    /// The MCU tunes: first without the extension channel, then with it
    /// (mt76x2_mcu_set_channel; the first answer is not checked).
    fn mcu_set_channel(&mut self, chan: u8, scan: bool) -> Result<()> {
        let mut m = [chan, u8::from(scan), 0, 0, 0, 0, 0, 0];
        m[4..6].copy_from_slice(&CHAINMASK.to_le_bytes());
        let _ = self.mcu_send(CMD_SWITCH_CHANNEL_OP, &m, true);
        self.usb.delay_us(5000);
        m[6] = 0xe0;
        self.mcu_send(CMD_SWITCH_CHANNEL_OP, &m, true)
    }

    fn mcu_init_gain(&mut self, chan: u8, gain: u32, force: bool) -> Result<()> {
        let c = u32::from(chan) | if force { 1 << 31 } else { 0 };
        self.mcu_send2(CMD_INIT_GAIN_OP, c, gain, true)
    }

    /// TSSI compensation: triggered, then given each chain's slope and
    /// offset (mt76x2_mcu_tssi_comp).
    fn mcu_tssi_comp(&mut self, pa_mode: u8, cal_mode: u8, slope: [u8; 2], offset: [u8; 2]) -> Result<()> {
        let mut m = [0; 12];
        m[..4].copy_from_slice(&CAL_TSSI_COMP.to_le_bytes());
        m[4] = pa_mode;
        m[5] = cal_mode;
        m[8..12].copy_from_slice(&[slope[0], slope[1], offset[0], offset[1]]);
        self.mcu_send(CMD_CALIBRATION_OP, &m, true)
    }

    /// The MCU reset (mt76x02u_mcu_fw_reset).
    fn mcu_fw_reset(&mut self) -> Result<()> {
        Ok(self.usb.vendor_out(REQ_DEV_MODE, DEV_MODE_RESET, 0, &[])?)
    }

    /// The USB DMA and the FCE set up to carry the MCU's code.
    fn fce_setup(&mut self) -> Result<()> {
        self.write(CFG_U3DMA, DMA_RX_BULK_EN | DMA_TX_BULK_EN | (0x20 & DMA_RX_BULK_AGG_TOUT))?;
        self.write(FCE_PSE_CTRL, 1)?;
        self.write(TX_CPU_FCE_BASE, 0x0040_0230)?;
        self.write(TX_CPU_FCE_MAX_COUNT, 1)?;
        self.write(FCE_PDMA, 0x44)?;
        self.write(FCE_SKIP_FS, 3)
    }

    /// `data` into the MCU's memory at `offset`, a block of up to
    /// `max_payload` less 8 per transfer after its address and length
    /// (mt76x02u_mcu_fw_send_data).
    fn fw_send_data(&mut self, data: &[u8], max_payload: usize, offset: u32) -> Result<()> {
        let mut pos = 0;
        for block in data.chunks(max_payload - 8) {
            let info = CPU_TX_PORT << MSG_PORT_SHIFT | block.len() as u32 | MSG_TYPE_CMD;
            let len = (block.len() + 3) & !3;
            let mut buf = Vec::with_capacity(4 + len + 4);
            buf.extend_from_slice(&info.to_le_bytes());
            buf.extend_from_slice(block);
            buf.resize(4 + len + 4, 0);
            self.write_fce(FCE_DMA_ADDR, offset + pos)?;
            self.write_fce(FCE_DMA_LEN, (len as u32) << 16)?;
            self.usb.bulk_out(self.eps.mcu, &buf)?;
            let idx = self.read(TX_CPU_FCE_DESC_IDX)?;
            self.write(TX_CPU_FCE_DESC_IDX, idx.wrapping_add(1))?;
            pos += block.len() as u32;
            self.usb.delay_us(5000);
        }
        Ok(())
    }

    /// An FCE register, a half per request (mt76u_single_wr).
    fn write_fce(&mut self, offset: u16, val: u32) -> Result<()> {
        self.usb.vendor_out(REQ_WRITE_FCE, val as u16, offset, &[])?;
        Ok(self.usb.vendor_out(REQ_WRITE_FCE, (val >> 16) as u16, offset + 2, &[])?)
    }

    /// The ROM patch, unless another driver of a sibling chip has applied
    /// it under the hardware semaphore (mt76x2u_mcu_load_rom_patch).
    fn load_rom_patch(&mut self, patch: &RomPatch) -> Result<()> {
        let protect = self.asic >> 16 != 0x7612;
        if protect {
            self.poll(MCU_SEMAPHORE_03, 1, 1, 60, 10_000, "the ROM patch semaphore")?;
        }
        let r = self.rom_patch(patch, protect);
        if protect {
            self.write(MCU_SEMAPHORE_03, 1)?;
        }
        r
    }

    fn rom_patch(&mut self, patch: &RomPatch, protect: bool) -> Result<()> {
        let (reg, mask) = if self.rev() >= REV_E3 { (MCU_CLOCK_CTL, 1) } else { (MCU_COM_REG0, 2) };
        if protect && self.read(reg)? & mask != 0 {
            return Ok(());
        }
        self.write(CFG_U3DMA, DMA_RX_BULK_EN | DMA_TX_BULK_EN | (0x20 & DMA_RX_BULK_AGG_TOUT))?;
        self.mcu_fw_reset()?;
        self.usb.delay_us(5000);
        self.fce_setup()?;
        self.fw_send_data(patch.patch, ROM_PATCH_MAX_PAYLOAD, ROM_PATCH_OFFSET)?;
        self.usb.class_out(REQ_DEV_MODE, DEV_MODE_IVB, 0, &PATCH_ENABLE)?;
        self.usb.class_out(REQ_DEV_MODE, DEV_MODE_IVB, 0, &PATCH_RESET_WMT)?;
        self.usb.delay_us(20_000);
        self.poll(reg, mask, mask, 10, 10_000, "the ROM patch")
    }

    /// The firmware: ILM, DLM, then the interrupt vectors start it
    /// (mt76x2u_mcu_load_firmware).
    fn load_firmware(&mut self, fw: &Firmware) -> Result<()> {
        self.mcu_fw_reset()?;
        self.usb.delay_us(5000);
        self.fce_setup()?;
        self.fw_send_data(fw.ilm, FW_MAX_PAYLOAD, self.chip.ilm_offset)?;
        let dlm = self.chip.dlm_offset + if self.rev() >= REV_E3 { DLM_OFFSET_E3 } else { 0 };
        self.fw_send_data(fw.dlm, FW_MAX_PAYLOAD, dlm)?;
        self.usb.vendor_out(REQ_DEV_MODE, DEV_MODE_IVB, 0, &[])?;
        self.poll(MCU_COM_REG0, 1, 1, 10, 10_000, "the firmware to start")?;
        self.set(MCU_COM_REG0, 1 << 1)?;
        self.write(FCE_PSE_CTRL, 1)
    }

    // ---- Bring-up --------------------------------------------------------

    /// The WLAN core reset and its clock on (mt76x2_reset_wlan).
    fn reset_wlan(&mut self) -> Result<()> {
        let mut val = self.read(WLAN_FUN_CTRL)? & !FRC_WL_ANT_SEL;
        if val & WLAN_EN != 0 {
            self.write(WLAN_FUN_CTRL, val | WLAN_RESET_RF)?;
            self.usb.delay_us(20);
            val &= !WLAN_RESET_RF;
        }
        self.write(WLAN_FUN_CTRL, val)?;
        self.usb.delay_us(20);
        let val = self.read(WLAN_FUN_CTRL)?;
        self.write(WLAN_FUN_CTRL, val | WLAN_EN | WLAN_CLK_EN)?;
        self.usb.delay_us(20);
        Ok(())
    }

    /// The WLAN power domain, the baseband, both RF units
    /// (mt76x2u_power_on).
    fn power_on(&mut self) -> Result<()> {
        self.set(CFG_MTC_CTRL, MTC_MTCMOS_PWR_UP)?;
        let up = MTC_STATE_UP | MTC_PWR_ACK | MTC_PWR_ACK_S;
        // Not checked, as there.
        let _ = self.poll(CFG_MTC_CTRL, up, up, 100, 10, "the WLAN power");
        self.clear(CFG_MTC_CTRL, 0x7f << 16)?;
        self.usb.delay_us(10);
        self.clear(CFG_MTC_CTRL, 0xf << 24)?;
        self.usb.delay_us(10);
        self.set(CFG_MTC_CTRL, 0xf << 24)?;
        self.clear(CFG_MTC_CTRL, 0xfff)?;
        // The AD/DA out of power down, the WLAN function on, the baseband
        // out of reset.
        self.clear(CFG_AD_DA, 1 << 3)?;
        self.set(CFG_WLAN_FUN, 1 << 0)?;
        self.clear(CFG_BBP_RESET, 1 << 18)?;
        self.power_on_rf(0)?;
        self.power_on_rf(1)
    }

    /// RF unit `unit`'s bandgap, LDO, AFE, ABB and ADDA (mt76x2u_power_on_rf
    /// and its patch).
    fn power_on_rf(&mut self, unit: u32) -> Result<()> {
        let shift = unit * 8;
        self.set(CFG_RF_POWER, 1 << shift)?;
        self.usb.delay_us(10);
        self.set(CFG_RF_POWER, (1 << 1 | 1 << 3 | 1 << 4 | 1 << 5) << shift)?;
        self.usb.delay_us(10);
        self.clear(CFG_RF_POWER, (1 << 2) << shift)?;
        self.usb.delay_us(10);
        self.set(CFG_RF_POWER, 1 << 0 | 1 << 16)?;
        self.usb.delay_us(1);
        self.clear(CFG_RF_PATCH_1C, 0xff)?;
        self.set(CFG_RF_PATCH_1C, 0x30)?;
        self.write(CFG_RF_PATCH_14, 0x484f)?;
        self.usb.delay_us(1);
        self.set(CFG_RF_POWER, 1 << 17)?;
        self.usb.delay_us(150);
        self.clear(CFG_RF_POWER, 1 << 16)?;
        self.usb.delay_us(50);
        self.set(CFG_RF_PATCH_14C, 1 << 19 | 1 << 20)?;
        self.set(0x530, 0xf)
    }

    /// Until the MAC answers (mt76x02_wait_for_mac).
    fn wait_for_mac(&mut self) -> Result<()> {
        for _ in 0..500 {
            let v = self.read(MAC_CSR0)?;
            if v != 0 && v != 0xffff_ffff {
                return Ok(());
            }
            self.usb.delay_us(5000);
        }
        Err(Error::Timeout("the MAC"))
    }

    /// The USB DMA: a frame a transfer, no receive aggregation
    /// (mt76x2u_init_dma).
    fn init_dma(&mut self) -> Result<()> {
        let val = self.read(CFG_U3DMA)?;
        self.write(CFG_U3DMA, (val | DMA_RX_DROP_OR_PAD | DMA_RX_BULK_EN | DMA_TX_BULK_EN) & !DMA_RX_BULK_AGG_EN)
    }

    /// The MAC's registers at their defaults (mt76x2u_mac_reset,
    /// mt76_write_mac_initvals, mt76x2u_mac_fixup_xtal).
    fn mac_reset(&mut self) -> Result<()> {
        self.write(WPDMA_GLO_CFG, 1 << 4 | 1 << 5)?;
        self.write(PBF_TX_MAX_PCNT, 0xefef_3f1f)?;
        self.write(PBF_RX_MAX_PCNT, 0xfebf)?;
        for &(reg, val) in INITVALS.iter().chain(PROT_VALS.iter()) {
            self.write(reg, val)?;
        }
        self.write(TX_LINK_CFG, 0x1020)?;
        self.write(AUTO_RSP_CFG, 0x13)?;
        self.write(MAX_LEN_CFG, 0x2f00)?;
        self.write(WMM_AIFSN, 0x2273)?;
        self.write(WMM_CWMIN, 0x2344)?;
        self.write(WMM_CWMAX, 0x34aa)?;
        self.clear(MAC_SYS_CTRL, RESET_CSR | RESET_BBP)?;
        if self.asic >> 16 == 0x7612 {
            self.clear(COEXCFG0, COEX_EN)?;
        }
        self.set(EXT_CCA_CFG, 0xf000)?;
        self.clear(TX_ALC_CFG_4, 1 << 31)?;
        self.fixup_xtal()
    }

    /// The crystal's trim from the EEPROM, SIFS, the FCE, the oscillator
    /// option (mt76x2u_mac_fixup_xtal).
    fn fixup_xtal(&mut self) -> Result<()> {
        let mut eep = self.ee16(EE_XTAL_TRIM_2);
        let mut offset = (eep & 0x7f) as i32;
        if eep & 0xff == 0xff {
            offset = 0;
        } else if eep & 0x80 != 0 {
            offset = -offset;
        }
        eep >>= 8;
        if eep == 0x00 || eep == 0xff {
            eep = self.ee16(EE_XTAL_TRIM_1) & 0xff;
            if eep == 0x00 || eep == 0xff {
                eep = 0x14;
            }
        }
        eep &= 0x7f;
        self.rmw_field(CFG_XO_CTRL5, XO_CTRL5_C2_VAL, (i32::from(eep) + offset) as u32)?;
        self.set(CFG_XO_CTRL6, XO_CTRL6_C2_CTRL)?;
        self.write(0x504, 0x0600_0000)?;
        self.write(0x50c, 0x0880_0000)?;
        self.usb.delay_us(5000);
        self.write(0x504, 0)?;
        // SIFS from 16 µs to 13.
        self.rmw_field(XIFS_TIME_CFG, 0xff << OFDM_SIFS_SHIFT, 0xd)?;
        self.rmw_field(BKOFF_SLOT_CFG, 0xf << CC_DELAY_SHIFT, 1)?;
        self.clear(FCE_L2_STUFF, L2_STUFF_WR_MPDU_LEN_EN)?;
        match (self.ee16(EE_NIC_CONF_2) >> NIC_CONF_2_XTAL_OPTION_SHIFT) & 3 {
            0 => self.write(XO_CTRL7, 0x5c1f_ee80),
            1 => self.write(XO_CTRL7, 0x5c1f_eed0),
            _ => Ok(()),
        }
    }

    /// The station's address, and the BSSID registers of an AP-client MAC
    /// (mt76x02_mac_setaddr).
    fn set_macaddr(&mut self, a: [u8; 6]) -> Result<()> {
        let lo = u32::from_le_bytes([a[0], a[1], a[2], a[3]]);
        let hi = u32::from(a[4]) | u32::from(a[5]) << 8;
        self.write(MAC_ADDR_DW0, lo)?;
        self.write(MAC_ADDR_DW1, hi | 0xff << U2ME_MASK_SHIFT)?;
        self.write(MAC_BSSID_DW0, lo)?;
        self.write(MAC_BSSID_DW1, hi | 3 << MBSS_MODE_SHIFT | MBSS_LOCAL_BIT)?;
        self.rmw(MAC_BSSID_DW1, 7 << MBEACON_N_SHIFT, 7 << MBEACON_N_SHIFT)?;
        for i in 0..8 {
            self.set_apc_bssid(i, [0; 6])?;
        }
        Ok(())
    }

    /// The station interface's BSSID (mt76x02_mac_set_bssid of bss index 8).
    fn set_bssid(&mut self, b: [u8; 6]) -> Result<()> {
        self.set_apc_bssid(VIF_IDX & 7, b)
    }

    fn set_apc_bssid(&mut self, idx: u8, b: [u8; 6]) -> Result<()> {
        let at = 8 * u32::from(idx);
        self.write(APC_BSSID_L0 + at, u32::from_le_bytes([b[0], b[1], b[2], b[3]]))?;
        self.rmw_field(APC_BSSID_H0 + at, 0xffff, u32::from(u16::from_le_bytes([b[4], b[5]])))
    }

    /// Station table entry `idx`: its bss index, its address
    /// (mt76x02_mac_wcid_setup).
    fn wcid_setup(&mut self, idx: u8, vif: u8, mac: Option<[u8; 6]>) -> Result<()> {
        let attr = u32::from(vif & 7) << WCID_ATTR_BSS_IDX_SHIFT | if vif & 8 != 0 { WCID_ATTR_BSS_IDX_EXT } else { 0 };
        self.write(wcid_attr(idx), attr)?;
        if idx >= 128 {
            return Ok(());
        }
        let mut addr = [0u8; 8];
        addr[..6].copy_from_slice(&mac.unwrap_or([0; 6]));
        self.write_copy(wcid_addr(idx), &addr)
    }

    /// Shared key `key` of bss index `vif`: no cipher, zeros
    /// (mt76x02_mac_shared_key_setup with no key).
    fn shared_key_clear(&mut self, vif: u8, key: u8) -> Result<()> {
        let (vif, key) = (u32::from(vif), u32::from(key));
        let (mode, skey) = if vif & 8 != 0 {
            (SKEY_MODE_BASE_1 + ((vif & 7) / 2) * 4, SKEY_BASE_1 + (4 * (vif & 7) + key) * 32)
        } else {
            (SKEY_MODE_BASE_0 + (vif / 2) * 4, SKEY_BASE_0 + (4 * vif + key) * 32)
        };
        self.clear(mode, 0xf << (4 * (key + 4 * (vif & 1))))?;
        self.write_copy(skey, &[0; 32])
    }

    /// No beacons of its own, the timer synchronized to the network's
    /// (mt76x02_init_beacon_config; the beacon slots of an access point
    /// are not set up).
    fn init_beacon_config(&mut self) -> Result<()> {
        self.clear(BEACON_TIME_CFG, BEACON_TIMER_EN | BEACON_TBTT_EN | BEACON_TX)?;
        self.set(BEACON_TIME_CFG, BEACON_SYNC_MODE)?;
        self.write(BCN_BYPASS_MASK, 0xffff)
    }

    /// Both chains receive (mt76x02_phy_set_rxpath).
    fn set_rxpath(&mut self) -> Result<()> {
        let mut val = self.read(bbp(BBP_AGC, 0))? & !(1 << 4);
        if CHAINMASK & 0xf == 2 {
            val |= 1 << 3;
        } else {
            val &= !(1 << 3);
        }
        self.write(bbp(BBP_AGC, 0), val)?;
        self.read(bbp(BBP_AGC, 0))?;
        Ok(())
    }

    /// Both chains transmit (mt76x02_phy_set_txdac).
    fn set_txdac(&mut self) -> Result<()> {
        if (CHAINMASK >> 8) & 0xf == 2 {
            self.set(bbp(BBP_TXBE, 5), 0x3)
        } else {
            self.clear(bbp(BBP_TXBE, 5), 0x3)
        }
    }

    /// The counters cleared, the MAC sending then hearing
    /// (mt76x02u_mac_start, mt76x02_mac_reset_counters).
    fn mac_start(&mut self) -> Result<()> {
        for reg in [RX_STAT_0, RX_STAT_1, RX_STAT_2, TX_STA_0, TX_STA_1, TX_STA_2] {
            self.read(reg)?;
        }
        for i in 0..16 {
            self.read(if i < 8 { TX_AGG_CNT_BASE0 + 4 * i } else { TX_AGG_CNT_BASE1 + 4 * (i - 8) })?;
        }
        for _ in 0..16 {
            self.read(TX_STAT_FIFO)?;
        }
        self.write(MAC_SYS_CTRL, ENABLE_TX)?;
        self.poll(WPDMA_GLO_CFG, TX_DMA_BUSY | RX_DMA_BUSY, 0, 20_000, 10, "the DMA engine")?;
        self.write(RX_FILTR_CFG, self.rxfilter)?;
        self.write(MAC_SYS_CTRL, ENABLE_TX | ENABLE_RX)?;
        self.poll(WPDMA_GLO_CFG, TX_DMA_BUSY | RX_DMA_BUSY, 0, 5, 10, "the DMA engine")
    }

    /// The MAC stopped, its queues and the USB DMA drained
    /// (mt76x2u_mac_stop).
    fn mac_stop_usb(&mut self) -> Result<()> {
        let rts = self.read(TX_RTS_CFG)?;
        self.write(TX_RTS_CFG, rts & !RTS_RETRY_LIMIT)?;
        self.clear(TXOP_CTRL_CFG, TXOP_ED_CCA_EN)?;
        self.clear(TXOP_HLDR_ET, TX40M_BLK_EN)?;
        for i in 0..2000 {
            if self.read(CFG_U3DMA)? & DMA_TX_BUSY == 0 && i > 10 {
                break;
            }
            self.usb.delay_us(50);
        }
        for _ in 0..200 {
            if self.read(PBF_TXQ_PCNT)? == 0 && self.read(PSE_PCNT0)? & 0xff == 0 && self.read(PSE_PCNT1)? & 0xff00_ff00 == 0 {
                break;
            }
            self.usb.delay_us(10);
        }
        self.clear(MAC_SYS_CTRL, ENABLE_RX | ENABLE_TX)?;
        let mut stopped = false;
        for _ in 0..1000 {
            if self.read(MAC_STATUS)? & STATUS_TX == 0 && self.read(bbp(BBP_IBI, 12))? == 0 {
                stopped = true;
                break;
            }
            self.usb.delay_us(10);
        }
        if !stopped {
            self.set(bbp(BBP_CORE, 4), 1 << 1)?;
            self.clear(bbp(BBP_CORE, 4), 1 << 1)?;
            self.set(bbp(BBP_CORE, 4), 1 << 0)?;
            self.clear(bbp(BBP_CORE, 4), 1 << 0)?;
        }
        let mut count = 0;
        for _ in 0..200 {
            if self.read(PBF_RXQ_PCNT)? & 0x00ff_0000 == 0 && self.read(PSE_PCNT0)? == 0 && self.read(PSE_PCNT1)? == 0 {
                count += 1;
                if count > 10 {
                    break;
                }
            }
            self.usb.delay_us(50_000);
        }
        // A MAC that does not stop hearing is not an error, as there.
        let _ = self.poll(MAC_STATUS, STATUS_RX, 0, 200, 10, "the MAC's RX");
        for i in 0..2000 {
            if self.read(CFG_U3DMA)? & DMA_RX_BUSY == 0 && i > 10 {
                break;
            }
            self.usb.delay_us(50);
        }
        self.write(TX_RTS_CFG, rts)
    }

    /// The MAC paused for a channel change (mt76x2_mac_stop, not forced).
    fn mac_stop(&mut self) -> Result<()> {
        self.clear(TXOP_CTRL_CFG, TXOP_ED_CCA_EN)?;
        self.clear(TXOP_HLDR_ET, TX40M_BLK_EN)?;
        self.write(MAC_SYS_CTRL, 0)?;
        let rts = self.read(TX_RTS_CFG)?;
        self.write(TX_RTS_CFG, rts & !RTS_RETRY_LIMIT)?;
        for _ in 0..300 {
            if self.read(MAC_STATUS)? & (STATUS_RX | STATUS_TX) == 0 && self.read(bbp(BBP_IBI, 12))? == 0 {
                break;
            }
            self.usb.delay_us(1);
        }
        self.write(TX_RTS_CFG, rts)
    }

    fn mac_resume(&mut self) -> Result<()> {
        self.write(MAC_SYS_CTRL, ENABLE_TX | ENABLE_RX)
    }

    /// The channel busy time counters restarted (mt76x02_mac_cc_reset).
    fn mac_cc_reset(&mut self) -> Result<()> {
        self.write(CH_TIME_CFG, CH_TIME_CFG_CC)?;
        self.read(CH_BUSY)?;
        self.read(CH_IDLE)?;
        Ok(())
    }

    /// The slot time, and the ACK timeout it and SIFS make
    /// (mt76x02_set_tx_ackto, coverage class 0).
    fn set_tx_ackto(&mut self, slot: u32) -> Result<()> {
        self.rmw_field(BKOFF_SLOT_CFG, SLOTTIME, slot)?;
        let sifs = self.get_field(XIFS_TIME_CFG, 0xff << OFDM_SIFS_SHIFT)?;
        self.rmw_field(TX_TIMEOUT_CFG, 0xff << ACKTO_SHIFT, slot + sifs)
    }

    // ---- Channels --------------------------------------------------------

    /// Channel `chan` with the MAC paused around it (mt76x2u_set_channel).
    fn tune(&mut self, chan: u8, scan: bool) -> Result<()> {
        if !self.channels().contains(&chan) {
            return Err(Error::Channel(chan));
        }
        self.chan = chan;
        self.mac_stop()?;
        let r = self.phy_set_channel(chan, scan);
        self.mac_cc_reset()?;
        self.mac_resume()?;
        r
    }

    /// Gains, power, band, the MCU's tuning and calibrations, then unless
    /// scanning the channel's own calibration (mt76x2u_phy_set_channel at
    /// 20 MHz).
    fn phy_set_channel(&mut self, chan: u8, scan: bool) -> Result<()> {
        let ghz5 = chan > 14;
        self.cal.channel_cal_done = false;
        self.read_rx_gain(chan);
        self.set_txpower_regs(ghz5)?;
        self.configure_tx_delay(ghz5)?;
        self.set_txpower(chan)?;
        self.set_band(ghz5)?;
        self.set_bw()?;
        self.rmw(EXT_CCA_CFG, EXT_CCA_MASK, EXT_CCA_20)?;
        self.mcu_set_channel(chan, scan)?;
        self.mcu_init_gain(chan, self.cal.mcu_gain, true)?;
        // LDPC received.
        if self.rev() >= REV_E3 {
            self.set(bbp(BBP_RXO, 13), 1 << 10)?;
        }
        if !self.cal.init_cal_done && self.ee16(EE_BT_RCAL_RESULT) as u8 != 0xff {
            self.mcu_calibrate(CAL_R, 0)?;
        }
        self.mcu_calibrate(CAL_RXDCOC, u32::from(chan))?;
        // The RX low-pass filter.
        if !self.cal.init_cal_done {
            self.mcu_calibrate(CAL_RC, 0)?;
        }
        self.cal.init_cal_done = true;
        self.write(bbp(BBP_AGC, 61), 0xff64_a4e2)?;
        self.write(bbp(BBP_AGC, 7), 0x0808_1010)?;
        self.write(bbp(BBP_AGC, 11), 0x0000_0404)?;
        self.write(bbp(BBP_AGC, 2), 0x0000_7070)?;
        self.write(TXOP_CTRL_CFG, 0x0410_1b3f)?;
        self.set(bbp(BBP_TXO, 4), 1 << 25)?;
        self.set(bbp(BBP_RXO, 13), 1 << 8)?;
        if scan {
            return Ok(());
        }
        self.channel_calibrate(true)?;
        self.init_agc_gain()?;
        if self.tssi_enabled() {
            self.rmw_field(TX_ALC_CFG_1, 0x3f, 0x38)?;
            self.rmw_field(TX_ALC_CFG_2, 0x3f, 0x38)?;
            if !radar(chan) {
                let flag = u32::from(ghz5) | if self.ext_pa(ghz5) { 1 << 8 } else { 0 };
                self.mcu_calibrate(CAL_TSSI, flag)?;
                self.cal.tssi_cal_done = true;
            }
        }
        Ok(())
    }

    /// The channel's calibrations, once, none on a radar channel
    /// (mt76x2u_phy_channel_calibrate).
    fn channel_calibrate(&mut self, mac_stopped: bool) -> Result<()> {
        if self.cal.channel_cal_done || radar(self.chan) {
            return Ok(());
        }
        let ghz5 = self.chan > 14;
        if !mac_stopped {
            self.mac_stop_usb()?;
        }
        if ghz5 {
            self.mcu_calibrate(CAL_LC, 0)?;
        }
        self.mcu_calibrate(CAL_TX_LOFT, u32::from(ghz5))?;
        self.mcu_calibrate(CAL_TXIQ, u32::from(ghz5))?;
        self.mcu_calibrate(CAL_RXIQC_FI, u32::from(ghz5))?;
        self.mcu_calibrate(CAL_TEMP_SENSOR, 0)?;
        self.mcu_calibrate(CAL_TX_SHAPING, 0)?;
        if !mac_stopped {
            self.mac_resume()?;
        }
        self.apply_gain_adj()?;
        self.edcca_init()?;
        self.cal.channel_cal_done = true;
        Ok(())
    }

    fn ext_pa(&self, ghz5: bool) -> bool {
        let conf0 = self.ee16(EE_NIC_CONF_0);
        conf0 & if ghz5 { NIC_CONF_0_PA_INT_5G } else { NIC_CONF_0_PA_INT_2G } == 0
    }

    fn ext_lna(&self, ghz5: bool) -> bool {
        let conf1 = self.ee16(EE_NIC_CONF_1);
        conf1 & if ghz5 { NIC_CONF_1_LNA_EXT_5G } else { NIC_CONF_1_LNA_EXT_2G } != 0
    }

    fn temp_tx_alc_enabled(&self) -> bool {
        self.ee16(EE_TX_POWER_EXT_PA_5G) & 1 << 15 != 0 && self.ee16(EE_NIC_CONF_1) & NIC_CONF_1_TEMP_TX_ALC != 0
    }

    fn tssi_enabled(&self) -> bool {
        !self.temp_tx_alc_enabled() && self.ee16(EE_NIC_CONF_1) & NIC_CONF_1_TX_ALC_EN != 0
    }

    /// The receive gains, signal offsets and LNA gain of channel `chan`
    /// from the EEPROM (mt76x2_read_rx_gain, mt76x02_get_rx_gain).
    fn read_rx_gain(&mut self, chan: u8) {
        let ghz5 = chan > 14;
        let val = if ghz5 { self.rx_gain_5g(chan) } else { (self.ee16(EE_RF_2G_RX_HIGH_GAIN) >> 8) as u8 };
        self.cal.high_gain =
            if valid(val) { [sign_extend(u32::from(val), 4) as i8, sign_extend(u32::from(val >> 4), 4) as i8] } else { [0, 0] };
        let lna = self.ee16(EE_LNA_GAIN);
        let lna_2g = lna as u8;
        let mut lna_5g = [(lna >> 8) as u8, (self.ee16(EE_RSSI_OFFSET_2G_1) >> 8) as u8, (self.ee16(EE_RSSI_OFFSET_5G_1) >> 8) as u8];
        for i in 1..3 {
            if !valid(lna_5g[i]) {
                lna_5g[i] = lna_5g[0];
            }
        }
        let offs = self.ee16(if ghz5 { EE_RSSI_OFFSET_5G_0 } else { EE_RSSI_OFFSET_2G_0 });
        for (chain, v) in [offs as u8, (offs >> 8) as u8].into_iter().enumerate() {
            self.cal.rssi_offset[chain] = if valid(v) { sign_extend_optional(u32::from(v), 7) as i8 } else { 0 };
        }
        self.cal.mcu_gain =
            u32::from(lna_2g) | u32::from(lna_5g[0]) << 8 | u32::from(lna_5g[1]) << 16 | u32::from(lna_5g[2]) << 24;
        let mut l = 0;
        if !self.ext_lna(ghz5) {
            l = if !ghz5 {
                lna_2g
            } else if chan <= 64 {
                lna_5g[0]
            } else if chan <= 128 {
                lna_5g[1]
            } else {
                lna_5g[2]
            };
            if l == 0xff {
                l = 0;
            }
        }
        self.cal.lna_gain = sign_extend(u32::from(l), 8) as i8;
    }

    /// The high gain byte of 5 GHz channel `chan`'s group.
    fn rx_gain_5g(&self, chan: u8) -> u8 {
        let (at, hi) = match cal_group(chan) {
            0 => (EE_RF_5G_GRP0_1_RX_HIGH_GAIN, false),
            1 => (EE_RF_5G_GRP0_1_RX_HIGH_GAIN, true),
            2 => (EE_RF_5G_GRP2_3_RX_HIGH_GAIN, false),
            3 => (EE_RF_5G_GRP2_3_RX_HIGH_GAIN, true),
            4 => (EE_RF_5G_GRP4_5_RX_HIGH_GAIN, false),
            _ => (EE_RF_5G_GRP4_5_RX_HIGH_GAIN, true),
        };
        let v = self.ee16(at);
        if hi { (v >> 8) as u8 } else { v as u8 }
    }

    /// The power amplifiers' registers for the band
    /// (mt76x2_phy_set_txpower_regs).
    fn set_txpower_regs(&mut self, ghz5: bool) -> Result<()> {
        let ext = self.ext_pa(ghz5);
        let pa_mode = if !ghz5 {
            self.write(TX_ALC_CFG_2, 0x3516_0a00)?;
            self.write(TX_ALC_CFG_3, 0x3516_0a06)?;
            let (a0, a1) = if ext { (0x0000_ec00, 0x0000_ec00) } else { (0xf400_0200, 0xfa00_0200) };
            self.write(RF_PA_MODE_ADJ0, a0)?;
            self.write(RF_PA_MODE_ADJ1, a1)?;
            [0x0100_55ff, 0x0055_0055]
        } else {
            let (c2, c3) = if ext { (0x2f0f_0400, 0x2f0f_0476) } else { (0x1b0f_0400, 0x1b0f_0476) };
            self.write(TX_ALC_CFG_2, c2)?;
            self.write(TX_ALC_CFG_3, c3)?;
            let adj = if ext { 0x0400_0000 } else { 0 };
            self.write(RF_PA_MODE_ADJ0, adj)?;
            self.write(RF_PA_MODE_ADJ1, adj)?;
            [0x0000_ffff, 0x00ff_00ff]
        };
        self.write(BB_PA_MODE_CFG0, pa_mode[0])?;
        self.write(BB_PA_MODE_CFG1, pa_mode[1])?;
        self.write(RF_PA_MODE_CFG0, pa_mode[0])?;
        self.write(RF_PA_MODE_CFG1, pa_mode[1])?;
        let (g0, g1, alc4) = match (ext, ghz5) {
            (true, false) => (0x3c3c_023c, 0x3c3c_023c, 0x1818),
            (true, true) => (0x363c_023c, 0x363c_023c, 0x1818),
            (false, false) => (0x0f3c_3c3c, 0x0f3c_3c3c, 0x0606),
            (false, true) => (0x383c_023c, 0x2428_2e28, 0),
        };
        self.write(TX0_RF_GAIN_CORR, g0)?;
        self.write(TX1_RF_GAIN_CORR, g1)?;
        self.write(TX_ALC_CFG_4, alc4)
    }

    /// The transmit delays of a 20 MHz channel, and SIFS
    /// (mt76x2_configure_tx_delay).
    fn configure_tx_delay(&mut self, ghz5: bool) -> Result<()> {
        let (cfg0, cfg1) = if self.ext_pa(ghz5) { (0x0010_1101, 0x0001_1414) } else { (0x0010_1001, 0x0002_1414) };
        self.write(TX_SW_CFG0, cfg0)?;
        self.write(TX_SW_CFG1, cfg1)?;
        self.rmw_field(XIFS_TIME_CFG, 0xff << OFDM_SIFS_SHIFT, 15)
    }

    /// Each rate's transmit power and the chains' initial power from the
    /// EEPROM, under the limit (mt76x2_phy_set_txpower,
    /// mt76x02_phy_set_txpower).
    fn set_txpower(&mut self, chan: u8) -> Result<()> {
        let txp = self.power_info(chan);
        let mut t = self.rate_power(chan);
        for p in t.all() {
            *p = (i32::from(*p) + i32::from(txp.target_power)) as i8;
            if i32::from(*p) > TXPOWER_CONF {
                *p = TXPOWER_CONF as i8;
            }
        }
        // The smallest set power (mt76x2_get_min_rate_power).
        let mut base = 0i32;
        for p in t.all() {
            if *p != 0 {
                base = if base != 0 { base.min(i32::from(*p)) } else { i32::from(*p) };
            }
        }
        let delta = base - i32::from(txp.target_power);
        let chain = |c: &ChainPower| i32::from(c.target_power) + i32::from(c.delta) + delta;
        let (mut txp0, mut txp1) = (chain(&txp.chain[0]), chain(&txp.chain[1]));
        let gain = txp0.min(txp1);
        if gain < 0 {
            base -= gain;
            txp0 -= gain;
            txp1 -= gain;
        } else if gain > 0x2f {
            base -= gain - 0x2f;
            txp0 = 0x2f;
            txp1 = 0x2f;
        }
        for p in t.all() {
            *p = (i32::from(*p) - base) as i8;
        }
        self.rmw_field(TX_ALC_CFG_0, 0x3f, txp0 as u32)?;
        self.rmw_field(TX_ALC_CFG_0, 0x3f00, txp1 as u32)?;
        let m = |a: i8, b: i8, c: i8, d: i8| {
            (a as u32 & 0x3f) | (b as u32 & 0x3f) << 8 | (c as u32 & 0x3f) << 16 | (d as u32 & 0x3f) << 24
        };
        self.write(TX_PWR_CFG_0, m(t.cck[0], t.cck[2], t.ofdm[0], t.ofdm[2]))?;
        self.write(TX_PWR_CFG_1, m(t.ofdm[4], t.ofdm[6], t.ht[0], t.ht[2]))?;
        self.write(TX_PWR_CFG_2, m(t.ht[4], t.ht[6], t.ht[8], t.ht[10]))?;
        self.write(TX_PWR_CFG_3, m(t.ht[12], t.ht[14], t.ht[0], t.ht[2]))?;
        self.write(TX_PWR_CFG_4, m(t.ht[4], t.ht[6], 0, 0))?;
        self.write(TX_PWR_CFG_7, m(t.ofdm[7], t.vht[0], t.ht[7], t.vht[1]))?;
        self.write(TX_PWR_CFG_8, m(t.ht[14], 0, t.vht[0], t.vht[1]))?;
        self.write(TX_PWR_CFG_9, m(t.ht[7], 0, t.vht[0], t.vht[1]))
    }

    /// Each rate's power offset from the EEPROM (mt76x2_get_rate_power).
    fn rate_power(&self, chan: u8) -> RatePower {
        let ghz5 = chan > 14;
        let mut t = RatePower::default();
        let pair = |v: u16| (rate_power_val(v as u8), rate_power_val((v >> 8) as u8));
        let (a, b) = pair(self.ee16(EE_TX_POWER_CCK));
        t.cck = [a, a, b, b];
        let (a, b) = pair(self.ee16(if ghz5 { EE_TX_POWER_OFDM_5G_6M } else { EE_TX_POWER_OFDM_2G_6M }));
        let (c, d) = pair(self.ee16(if ghz5 { EE_TX_POWER_OFDM_5G_24M } else { EE_TX_POWER_OFDM_2G_24M }));
        t.ofdm = [a, a, b, b, c, c, d, d];
        for (i, at) in [EE_TX_POWER_HT_MCS0, EE_TX_POWER_HT_MCS4, EE_TX_POWER_HT_MCS8, EE_TX_POWER_HT_MCS12].into_iter().enumerate() {
            let (a, b) = pair(self.ee16(at));
            t.ht[4 * i..4 * i + 4].copy_from_slice(&[a, a, b, b]);
        }
        let mut v = self.ee16(EE_TX_POWER_VHT_MCS8);
        if !ghz5 {
            v >>= 8;
        }
        let p = rate_power_val((v >> 8) as u8);
        t.vht = [p, p];
        t
    }

    /// Each chain's power calibration for channel `chan`, and the target
    /// power (mt76x2_get_power_info and its _2g, _5g).
    fn power_info(&self, chan: u8) -> PowerInfo {
        let mut t = PowerInfo::default();
        if chan > 14 {
            let delta_idx = match chan {
                192.. => 4,
                184.. => 3,
                ..44 => 3,
                ..52 => 4,
                ..58 => 3,
                ..98 => 4,
                ..106 => 3,
                ..116 => 4,
                ..130 => 3,
                ..149 => 4,
                ..157 => 3,
                _ => 4,
            };
            let group = cal_group(chan) * TX_POWER_GROUP_SIZE_5G;
            for (c, start) in [EE_TX_POWER_0_START_5G, EE_TX_POWER_1_START_5G].into_iter().enumerate() {
                t.chain[c] = self.chain_power(start + group, delta_idx);
            }
            t.target_power = self.ee16(EE_RF_2G_RX_HIGH_GAIN) as u8;
        } else {
            let delta_idx = match chan {
                ..6 => 3,
                ..11 => 4,
                _ => 5,
            };
            for (c, start) in [EE_TX_POWER_0_START_2G, EE_TX_POWER_1_START_2G].into_iter().enumerate() {
                t.chain[c] = self.chain_power(start, delta_idx);
            }
            t.target_power = (self.ee16(EE_RF_2G_TSSI_OFF_TXPOWER) >> 8) as u8;
        }
        if self.tssi_enabled() || !valid(t.target_power) {
            t.target_power = t.chain[0].target_power as u8;
        }
        t
    }

    fn chain_power(&self, at: usize, delta_idx: usize) -> ChainPower {
        let d = &self.ee[at..at + 6];
        ChainPower {
            tssi_slope: d[0],
            tssi_offset: d[1],
            target_power: d[2] as i8,
            delta: sign_extend_optional(u32::from(d[delta_idx]), 7) as i8,
        }
    }

    /// The band (mt76x02_phy_set_band, the primary channel lower).
    fn set_band(&mut self, ghz5: bool) -> Result<()> {
        if ghz5 {
            self.clear(TX_BAND_CFG, BAND_2G)?;
            self.set(TX_BAND_CFG, BAND_5G)?;
        } else {
            self.set(TX_BAND_CFG, BAND_2G)?;
            self.clear(TX_BAND_CFG, BAND_5G)?;
        }
        self.rmw_field(TX_BAND_CFG, BAND_UPPER_40M, 0)
    }

    /// 20 MHz (mt76x02_phy_set_bw).
    fn set_bw(&mut self) -> Result<()> {
        self.rmw_field(bbp(BBP_CORE, 1), CORE_R1_BW, 0)?;
        self.rmw_field(bbp(BBP_AGC, 0), AGC_R0_BW, 1)?;
        self.rmw_field(bbp(BBP_AGC, 0), AGC_R0_CTRL_CHAN, 0)?;
        self.rmw_field(bbp(BBP_TXBE, 0), TXBE_R0_CTRL_CHAN, 0)
    }

    /// The EEPROM's gain adjustments into the AGC (mt76x2_apply_gain_adj).
    fn apply_gain_adj(&mut self) -> Result<()> {
        for (reg, adj) in [(4, self.cal.high_gain[0]), (5, self.cal.high_gain[1])] {
            let gain = self.get_field(bbp(BBP_AGC, reg), AGC_LNA_HIGH_GAIN)? as i8;
            self.rmw_field(bbp(BBP_AGC, reg), AGC_LNA_HIGH_GAIN, gain.wrapping_sub(adj / 2) as u8 as u32)?;
        }
        for (reg, adj) in [(8, self.cal.high_gain[0]), (9, self.cal.high_gain[1])] {
            let gain = self.get_field(bbp(BBP_AGC, reg), AGC_GAIN)? as i8;
            self.rmw_field(bbp(BBP_AGC, reg), AGC_GAIN, gain.wrapping_add(adj) as u8 as u32)?;
        }
        Ok(())
    }

    /// Energy detection off: CCA as usual, the PA and LNA on, the MAC
    /// sending (mt76x02_edcca_init with ed_monitor off, as it is outside
    /// ETSI's countries).
    fn edcca_init(&mut self) -> Result<()> {
        self.set(TX_LINK_CFG, TX_CFACK_EN)?;
        self.clear(TXOP_CTRL_CFG, TXOP_ED_CCA_EN)?;
        self.write(bbp(BBP_AGC, 2), 0x0000_7070)?;
        self.set(TXOP_HLDR_ET, TX40M_BLK_EN)?;
        self.set(MAC_SYS_CTRL, ENABLE_TX)?;
        self.set(AUTO_RSP_CFG, AUTO_RSP_EN)?;
        self.set(TX_PIN_CFG, TX_PIN_ON)?;
        self.read(ED_CCA_TIMER)?;
        Ok(())
    }

    /// The AGC's gains as calibrated (mt76x02_init_agc_gain).
    fn init_agc_gain(&mut self) -> Result<()> {
        self.cal.agc_gain_init = [
            self.get_field(bbp(BBP_AGC, 8), AGC_GAIN)? as u8,
            self.get_field(bbp(BBP_AGC, 9), AGC_GAIN)? as u8,
        ];
        self.cal.agc_gain_cur = self.cal.agc_gain_init;
        self.cal.low_gain = -1;
        self.cal.gain_init_done = true;
        Ok(())
    }

    /// The TSSI compensation: triggered, then once the baseband has
    /// measured, given the chains' slopes and offsets; then the DPD
    /// calibration, once (mt76x2_phy_tssi_compensate).
    fn tssi_compensate(&mut self) -> Result<()> {
        if !self.cal.tssi_cal_done {
            return Ok(());
        }
        if !self.cal.tssi_comp_pending {
            self.mcu_tssi_comp(0, 1 << 0, [0; 2], [0; 2])?;
            self.cal.tssi_comp_pending = true;
            return Ok(());
        }
        if self.read(bbp(BBP_CORE, 34))? & 1 << 4 != 0 {
            return Ok(());
        }
        self.cal.tssi_comp_pending = false;
        let txp = self.power_info(self.chan);
        let pa_mode = u8::from(self.ext_pa(self.chan > 14));
        let c = &txp.chain;
        self.mcu_tssi_comp(pa_mode, 1 << 1, [c[0].tssi_slope, c[1].tssi_slope], [c[0].tssi_offset, c[1].tssi_offset])?;
        if pa_mode != 0 || self.cal.dpd_cal_done {
            return Ok(());
        }
        self.usb.delay_us(10_000);
        self.mcu_calibrate(CAL_DPD, u32::from(self.chan))?;
        self.cal.dpd_cal_done = true;
        Ok(())
    }

    /// The receive gain for the access point's signal: lower when it is
    /// strong, and the VGA's adjusted to the false CCAs
    /// (mt76x2_phy_update_channel_gain at 20 MHz).
    fn update_channel_gain(&mut self) -> Result<()> {
        if !self.cal.gain_init_done {
            return Ok(());
        }
        let avg = self.avg_rssi.unwrap_or(-75);
        // mt76x02_get_rssi_gain_thresh and _low_ at 20 MHz.
        let low_gain = i8::from(avg > -68) + i8::from(avg > -82);
        let change = self.cal.low_gain < 0 || (self.cal.low_gain & 2) ^ (low_gain & 2) != 0;
        self.cal.low_gain = low_gain;
        if !change {
            if self.adjust_vga_gain()? {
                self.set_gain_val()?;
            }
            return Ok(());
        }
        let ghz5 = self.chan > 14;
        self.write(bbp(BBP_RXO, 14), 0x0056_0423)?;
        let low_gain_delta = if self.ext_lna(ghz5) { 10 } else { 14 };
        let mut agc_37 = 0x2121_262c;
        let agc_35 = if !ghz5 {
            0x1111_1516
        } else if low_gain == 2 {
            agc_37 = 0x0808_0808;
            0x0808_0808
        } else {
            0x1111_1116
        };
        let gain_delta = if low_gain == 2 {
            self.write(bbp(BBP_RXO, 18), 0xf000_a990)?;
            self.write(bbp(BBP_AGC, 35), 0x0808_0808)?;
            self.write(bbp(BBP_AGC, 37), 0x0808_0808)?;
            self.cal.agc_gain_adjust = 0;
            low_gain_delta
        } else {
            self.write(bbp(BBP_RXO, 18), 0xf000_a991)?;
            self.cal.agc_gain_adjust = low_gain_delta;
            0
        };
        self.write(bbp(BBP_AGC, 35), agc_35)?;
        self.write(bbp(BBP_AGC, 37), agc_37)?;
        for i in 0..2 {
            self.cal.agc_gain_cur[i] = self.cal.agc_gain_init[i].wrapping_sub(gain_delta);
        }
        self.set_gain_val()?;
        // The false CCA counter cleared.
        self.read(RX_STAT_1)?;
        Ok(())
    }

    /// The VGA's gain against false CCAs (mt76x02_phy_adjust_vga_gain).
    fn adjust_vga_gain(&mut self) -> Result<bool> {
        let limit = if self.cal.low_gain > 0 { 16 } else { 4 };
        let false_cca = self.read(RX_STAT_1)? & 0xffff;
        let adj = &mut self.cal.agc_gain_adjust;
        if false_cca > 800 && *adj < limit {
            *adj += 2;
            Ok(true)
        } else if (false_cca < 10 && *adj > 0) || (*adj >= limit && false_cca < 500) {
            *adj -= 2;
            Ok(true)
        } else {
            Ok(false)
        }
    }

    /// The AGC gains of both chains (mt76x2_phy_set_gain_val at 20 MHz;
    /// a radar channel's AGC for detecting radar is an access point's).
    fn set_gain_val(&mut self) -> Result<()> {
        // An external LNA on 2.4 GHz has its own.
        let ghz5 = self.chan > 14;
        let val = if !ghz5 && self.ext_lna(false) { 0x0f36 << 16 } else { 0x1836 << 16 } | 0xf8;
        for (i, reg) in [8, 9].into_iter().enumerate() {
            let g = self.cal.agc_gain_cur[i].wrapping_sub(self.cal.agc_gain_adjust);
            self.write(bbp(BBP_AGC, reg), val | (u32::from(g) << 8) & AGC_GAIN)?;
        }
        Ok(())
    }
}

/// The MAC's registers at their defaults, from MediaTek's reference
/// driver (mt76_write_mac_initvals).
#[rustfmt::skip]
const INITVALS: [(u32, u32); 54] = [
    (PBF_SYS_CTRL, 0x0008_0c00), (PBF_CFG, 0x1efe_bcff), (FCE_PSE_CTRL, 0x0000_0001),
    (MAC_SYS_CTRL, 0x0000_0000), (MAX_LEN_CFG, 0x003e_3f00), (AMPDU_MAX_LEN_20M1S, 0xaaa9_9887),
    (AMPDU_MAX_LEN_20M2S, 0x0000_00aa), (XIFS_TIME_CFG, 0x33a4_0d0a), (BKOFF_SLOT_CFG, 0x0000_0209),
    (TBTT_SYNC_CFG, 0x0042_2010), (PWR_PIN_CFG, 0x0000_0000), (0x1238, 0x0017_00c8),
    (TX_SW_CFG0, 0x0010_1001), (TX_SW_CFG1, 0x0001_0000), (TX_SW_CFG2, 0x0000_0000),
    (TXOP_CTRL_CFG, 0x0400_583f), (TX_RTS_CFG, 0x00ff_ff20), (TX_TIMEOUT_CFG, 0x000a_2290),
    (TX_RETRY_CFG, 0x47f0_1f0f), (EXP_ACK_TIME, 0x002c_00dc), (TX_PROT_CFG6, 0xe3f4_2004),
    (TX_PROT_CFG7, 0xe3f4_2084), (TX_PROT_CFG8, 0xe3f4_2104), (PIFS_TX_CFG, 0x0006_0fff),
    (RX_FILTR_CFG, 0x0001_5f97), (LEGACY_BASIC_RATE, 0x0000_017f), (HT_BASIC_RATE, 0x0000_4003),
    (PN_PAD_MODE, 0x0000_0003), (TXOP_HLDR_ET, 0x0000_0002), (0xa44, 0x0000_0000),
    (HEADER_TRANS_CTRL, 0x0000_0000), (TSO_CTRL, 0x0000_0000), (AUX_CLK_CFG, 0x0000_0000),
    (DACCLK_EN_DLY_CFG, 0x0000_0000), (TX_ALC_CFG_4, 0x0000_0000), (TX_ALC_VGA3, 0x0000_0000),
    (TX_PWR_CFG_0, 0x3a3a_3a3a), (TX_PWR_CFG_1, 0x3a3a_3a3a), (TX_PWR_CFG_2, 0x3a3a_3a3a),
    (TX_PWR_CFG_3, 0x3a3a_3a3a), (TX_PWR_CFG_4, 0x3a3a_3a3a), (TX_PWR_CFG_7, 0x3a3a_3a3a),
    (TX_PWR_CFG_8, 0x0000_003a), (TX_PWR_CFG_9, 0x0000_003a), (EFUSE_CTRL, 0x0000_d000),
    (PAUSE_ENABLE_CONTROL1, 0x0000_000a), (FCE_WLAN_FLOW_CONTROL1, 0x6040_1c18), (WPDMA_DELAY_INT_CFG, 0x94ff_0000),
    (TX_SW_CFG3, 0x0000_0004), (HT_FBK_TO_LEGACY, 0x0000_1818), (VHT_HT_FBK_CFG1, 0xedcb_a980),
    (PROT_AUTO_TX_CFG, 0x0083_0083), (HT_CTRL_CFG, 0x0000_01ff), (TX_LINK_CFG, 0x0000_1020),
];

/// The protection of each mode: CCK, OFDM, mixed and green field at 20
/// and 40 MHz (DEFAULT_PROT_CFG_*: the rate, control, NAV, TXOP, RTS
/// threshold fields).
#[rustfmt::skip]
const PROT_VALS: [(u32, u32); 6] = [
    (CCK_PROT_CFG, 0x07f4_0003), (OFDM_PROT_CFG, 0x07f4_2004), (MM20_PROT_CFG, 0x0175_2004),
    (MM40_PROT_CFG, 0x03f5_2084), (GF20_PROT_CFG, 0x0175_2004), (GF40_PROT_CFG, 0x03f5_2084),
];

/// A register (__mt76u_rr): the request by its address space.
fn read<U: Usb>(usb: &mut U, addr: u32) -> Result<u32> {
    let req = match addr & TYPE_MASK {
        TYPE_EEPROM => REQ_READ_EEPROM,
        TYPE_CFG => REQ_READ_CFG,
        _ => REQ_MULTI_READ,
    };
    let a = addr & !TYPE_MASK;
    let mut b = [0; 4];
    usb.vendor_in(req, (a >> 16) as u16, a as u16, &mut b)?;
    Ok(u32::from_le_bytes(b))
}

/// The 5 GHz calibration group of channel `chan` (mt76x2_get_cal_channel_group:
/// Japan, UNII-1, UNII-2, UNII-2E's halves, UNII-3).
fn cal_group(chan: u8) -> usize {
    match chan {
        184..=196 => 0,
        ..=48 => 1,
        ..=64 => 2,
        ..=114 => 3,
        ..=144 => 4,
        _ => 5,
    }
}

/// An EEPROM byte that is set (mt76x02_field_valid).
fn valid(v: u8) -> bool {
    v != 0 && v != 0xff
}

/// A sign and magnitude value of `size` bits, the sign bit set for a
/// positive one (mt76x02_sign_extend).
fn sign_extend(val: u32, size: u32) -> i32 {
    let sign = val & 1 << (size - 1) != 0;
    let v = (val & ((1 << (size - 1)) - 1)) as i32;
    if sign { v } else { -v }
}

/// The same with an enable bit above it (mt76x02_sign_extend_optional).
fn sign_extend_optional(val: u32, size: u32) -> i32 {
    if val & 1 << size != 0 { sign_extend(val, size) } else { 0 }
}

fn rate_power_val(v: u8) -> i8 {
    if valid(v) { sign_extend_optional(u32::from(v), 7) as i8 } else { 0 }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn eeprom_values() {
        // Sign and magnitude, the sign bit set for positive.
        assert_eq!((sign_extend(0x8, 4), sign_extend(0x3, 4), sign_extend(0xc5, 8)), (0, -3, 0x45));
        assert_eq!((sign_extend_optional(0xc2, 7), sign_extend_optional(0x82, 7), sign_extend_optional(0x42, 7)), (2, -2, 0));
        assert_eq!((rate_power_val(0xff), rate_power_val(0xc4)), (0, 4));
        assert_eq!([36, 52, 100, 140, 149].map(cal_group), [1, 2, 3, 4, 5]);
        assert!(radar(52) && radar(144) && !radar(48) && !radar(149));
    }
}
