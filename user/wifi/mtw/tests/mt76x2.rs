//! The MT7612U's driver (`mtw::mt76x2`, what the `wifi` daemon runs on
//! one) against a model of the device (`model/mt76x2.rs`): its bring-up
//! with the ROM patch and firmware, the channels of both bands, a scan
//! of the model's air, joining an access point on each band, sending,
//! the rates AMRR picks over two streams, the periodic calibration and
//! leaving, with the model's checks on everything it does.

#[path = "../../iwx/tests/model/air.rs"]
mod air;
#[path = "model/mt76x2.rs"]
mod device;

use mtw::chip::{self, MT7612U};
use mtw::dev::{Error, Peer};
use mtw::mt76x2::dev::{self as x2, Dev};
use mtw::mt76x2::fw::{self, Firmware, RomPatch};
use mtw::mt76x2::reg::*;
use mtw::rate::TxRate;
use mtw::usb::{Endpoints, Usb, UsbError};

use device::{Device, AGC_GAIN_RESET, BULK, EP_DATA, MAC};

/// The driver's platform: the model's endpoints.
struct Host(Device);

impl Usb for Host {
    fn vendor_out(&mut self, request: u8, value: u16, index: u16, data: &[u8]) -> Result<(), UsbError> {
        self.0.vendor_out(request, value, index, data);
        Ok(())
    }
    fn vendor_in(&mut self, request: u8, value: u16, index: u16, data: &mut [u8]) -> Result<(), UsbError> {
        self.0.vendor_in(request, value, index, data);
        Ok(())
    }
    fn class_out(&mut self, request: u8, value: u16, index: u16, data: &[u8]) -> Result<(), UsbError> {
        self.0.class_out(request, value, index, data);
        Ok(())
    }
    fn bulk_out(&mut self, endpoint: u8, data: &[u8]) -> Result<(), UsbError> {
        self.0.bulk_out(endpoint, data);
        Ok(())
    }
    fn bulk_in(&mut self, endpoint: u8, buf: &mut [u8], _timeout_ms: u32) -> Result<usize, UsbError> {
        Ok(self.0.bulk_in(endpoint, buf))
    }
    fn delay_us(&mut self, _us: u32) {}
}

/// Synthetic files: a ROM patch of a few blocks, a firmware of an ILM
/// longer than a transfer and a DLM.
fn files() -> (Vec<u8>, Vec<u8>) {
    let patch: Vec<u8> = (0..5000u32).map(|i| (i * 3 + 1) as u8).collect();
    let ilm: Vec<u8> = (0..40_000u32).map(|i| (i * 7 + 3) as u8).collect();
    let dlm: Vec<u8> = (0..9000u32).map(|i| (i * 5 + 2) as u8).collect();
    (fw::build_patch(&patch), fw::build(&ilm, &dlm))
}

/// The radio attached and brought up.
fn up() -> Dev<Host> {
    let (p, f) = files();
    let (patch, fw) = (RomPatch::parse(&p).unwrap(), Firmware::parse(&f).unwrap());
    let model = Device::new(patch.patch, fw.ilm, fw.dlm);
    let mut dev = Dev::new(Host(model), &MT7612U, Endpoints::new(&BULK).unwrap()).unwrap();
    let info = dev.attach(&patch, &fw).unwrap();
    assert_eq!(info.mac, MAC);
    assert_eq!((info.mac_ver, info.rf_rev, info.ntx, info.nrx), (0x7612, 0x7612, 2, 2));
    assert_eq!(info.fw, Some((fw.build_ver, fw.fw_ver)));
    dev.init(info.mac).unwrap();
    dev
}

/// What the radio hears on the channel it is on now: {frame, dBm}.
fn hear(dev: &mut Dev<Host>) -> Vec<(Vec<u8>, i8)> {
    let ep = dev.rx_endpoint();
    let mut heard = Vec::new();
    for mut x in dev.usb.0.rx(ep) {
        dev.frames(&mut x, |f, dbm| heard.push((f.to_vec(), dbm)));
    }
    heard
}

/// The body of the first information element `id` of a beacon.
fn ie(beacon: &[u8], id: u8) -> Option<&[u8]> {
    let mut ies = &beacon[36..];
    while ies.len() >= 2 {
        let len = usize::from(ies[1]);
        if ies[0] == id {
            return ies.get(2..2 + len);
        }
        ies = &ies[2 + len..];
    }
    None
}

/// The access point a beacon describes, as the daemon reads it.
fn peer(beacon: &[u8]) -> Peer {
    let cap = u16::from_le_bytes([beacon[34], beacon[35]]);
    Peer {
        bssid: beacon[16..22].try_into().unwrap(),
        channel: ie(beacon, 3).unwrap()[0],
        rates: [ie(beacon, 1).unwrap_or(&[]), ie(beacon, 50).unwrap_or(&[])].concat(),
        ht_mcs: ie(beacon, 45).map(|ht| u16::from_le_bytes([ht[3], ht[4]])),
        short_preamble: cap & 0x20 != 0,
        short_slot: cap & 0x400 != 0,
    }
}

fn ssid(beacon: &[u8]) -> &[u8] {
    let ie = &beacon[36..];
    &ie[2..2 + usize::from(ie[1])]
}

/// The beacon of the network `name` on `chan`.
fn beacon(dev: &mut Dev<Host>, chan: u8, name: &[u8]) -> Vec<u8> {
    dev.set_chan(chan).unwrap();
    hear(dev).into_iter().map(|(f, _)| f).find(|f| f[0] == 0x80 && ssid(f) == name).expect("its beacon")
}

/// A management frame `subtype` from the station to `bssid`.
fn mgmt(subtype: u8, bssid: [u8; 6], body: &[u8]) -> Vec<u8> {
    let mut f = vec![subtype, 0, 0, 0];
    f.extend_from_slice(&bssid);
    f.extend_from_slice(&MAC);
    f.extend_from_slice(&bssid);
    f.extend_from_slice(&[0, 0]);
    f.extend_from_slice(body);
    f
}

/// Join network `name` on `chan` (802.11n when `ht`): its peer.
fn join(dev: &mut Dev<Host>, chan: u8, name: &[u8], ht: bool) -> Peer {
    let p = peer(&beacon(dev, chan, name));
    dev.joining(&p).unwrap();
    assert_eq!(dev.usb.0.bssid(), p.bssid);
    assert_eq!(dev.usb.0.tuned, Some((chan, false)));
    dev.send(&mgmt(0xb0, p.bssid, &[0, 0, 1, 0, 0, 0])).unwrap();
    let auth = hear(dev).into_iter().find(|(f, _)| f[0] == 0xb0).expect("authentication answered");
    assert_eq!(&auth.0[24..30], &[0, 0, 2, 0, 0, 0]);
    dev.send(&mgmt(0x00, p.bssid, &[0x01, 0x00, 10, 0, 0, 9])).unwrap();
    let assoc = hear(dev).into_iter().find(|(f, _)| f[0] == 0x10).expect("association answered");
    let aid = u16::from_le_bytes([assoc.0[28], assoc.0[29]]) & 0x3fff;
    dev.associated(&p, aid, ht).unwrap();
    assert_eq!(dev.usb.0.bytes(wcid_addr(1), 6), &p.bssid);
    p
}

/// A QoS data frame of priority 0 from the station to the distribution
/// system through `bssid` (its header padded on the way out).
fn data(bssid: [u8; 6]) -> Vec<u8> {
    let mut f = vec![0x88, 0x01, 0, 0];
    f.extend_from_slice(&bssid);
    f.extend_from_slice(&MAC);
    f.extend_from_slice(&[0xff; 6]);
    f.extend_from_slice(&[0, 0, 0, 0]);
    f.extend_from_slice(&[0xaa, 0xaa, 3, 0, 0, 0, 0x08, 0x00, 0x45]);
    f
}

/// `seconds` of traffic: 20 data frames to `bssid` a second, then the
/// once-a-second calibration; the rates AMRR chose, second by second.
fn traffic(dev: &mut Dev<Host>, bssid: [u8; 6], seconds: usize) -> Vec<TxRate> {
    let mut rates = Vec::new();
    for _ in 0..seconds {
        for _ in 0..20 {
            dev.send(&data(bssid)).unwrap();
        }
        dev.calibrate().unwrap();
        rates.push(dev.tx_rate());
    }
    rates
}

#[test]
fn bring_up() {
    let dev = up();
    let m = &dev.usb.0;
    assert!(m.patched && m.running && m.radio_on);
    assert_eq!(m.mac_addr(), MAC);
    assert_eq!(m.tuned, Some((1, false)));
    // A station's receive filter (control frames and others' unicast
    // dropped), the MAC on, no receive aggregation.
    assert_eq!(m.reg(RX_FILTR_CFG), 0x0001_7f97);
    assert_eq!(m.reg(MAC_SYS_CTRL), ENABLE_TX | ENABLE_RX);
    assert_eq!(m.cfg(CFG_U3DMA) & (DMA_RX_BULK_AGG_EN | DMA_RX_BULK_EN | DMA_TX_BULK_EN), DMA_RX_BULK_EN | DMA_TX_BULK_EN);
    // The MCU's commands: queue select, radio on, its settings, then the
    // first channel: tuned twice, its gain, calibrated.
    let cmds: Vec<u8> = m.commands.iter().map(|(c, _)| *c).take(5).collect();
    assert_eq!(cmds, [CMD_FUN_SET_OP, CMD_POWER_SAVING_OP, CMD_LOAD_CR, CMD_SWITCH_CHANNEL_OP, CMD_SWITCH_CHANNEL_OP]);
    let cals: Vec<u32> = m.calibrations().iter().map(|c| c.0).collect();
    assert_eq!(cals, [CAL_R, CAL_RXDCOC, CAL_RC, CAL_TX_LOFT, CAL_TXIQ, CAL_RXIQC_FI, CAL_TEMP_SENSOR, CAL_TX_SHAPING, CAL_TSSI]);
    // The chains' power from the EEPROM's targets (16 and 14 dBm), the
    // PA and LNA on.
    assert_eq!(m.reg(TX_ALC_CFG_0) & 0x3f3f, 0x1c20);
    assert_eq!(m.reg(TX_PIN_CFG) & TX_PIN_ON, TX_PIN_ON);
    assert_eq!(m.reg(TX_BAND_CFG) & (BAND_2G | BAND_5G), BAND_2G);
}

#[test]
fn other_chip() {
    let (p, f) = files();
    let (patch, fw) = (RomPatch::parse(&p).unwrap(), Firmware::parse(&f).unwrap());
    let mut model = Device::new(patch.patch, fw.ilm, fw.dlm);
    model.set_asic(0x7601_0001);
    let r = Dev::new(Host(model), &MT7612U, Endpoints::new(&BULK).unwrap());
    assert_eq!(r.err(), Some(Error::Chip(0x7601)));
    assert_eq!(chip::Chip::by_id((0x0e8d, 0x7612)), Some(&MT7612U));
}

#[test]
fn channels_and_scan() {
    let mut dev = up();
    assert_eq!(dev.channels().len(), 13 + 25);
    assert_eq!(dev.set_chan(14).err(), Some(Error::Channel(14)));
    for (chan, name, dbm) in [(1, &b"myos-open"[..], -40), (6, b"myos-wpa2", -55), (11, b"myos-far", -82), (36, b"myos-5g", -45)] {
        let before = dev.usb.0.calibrations().len();
        dev.set_chan(chan).unwrap();
        assert_eq!(dev.usb.0.tuned, Some((chan, true)));
        // Scanning: the receiver's DC offset calibrated, nothing else.
        assert_eq!(dev.usb.0.calibrations()[before..], [(CAL_RXDCOC, u32::from(chan))]);
        // A wildcard probe request goes out on the voice endpoint at the
        // band's slowest rate, wanting no ACK.
        let mut probe = mgmt(0x40, [0xff; 6], &[0, 0, 1, 4, 0x02, 0x04, 0x0b, 0x16]);
        probe[16..22].copy_from_slice(&[0xff; 6]);
        dev.send(&probe).unwrap();
        let s = dev.usb.0.sent.last().unwrap();
        let slowest = if chan > 14 { PHY_OFDM << RATE_PHY_SHIFT } else { PHY_CCK << RATE_PHY_SHIFT };
        assert_eq!((s.channel, s.endpoint, s.rate, s.ack_ctl & TXWI_ACK_REQ), (chan, EP_DATA[3], slowest, 0));
        let heard = hear(&mut dev);
        let beacons: Vec<_> = heard.iter().filter(|(f, _)| f[0] == 0x80).collect();
        assert_eq!(beacons.len(), 1, "channel {chan}");
        assert_eq!((ssid(&beacons[0].0), beacons[0].1), (name, dbm));
    }
}

#[test]
fn join_send_leave_5ghz() {
    let mut dev = up();
    let before = dev.usb.0.calibrations().len();
    let p = join(&mut dev, 36, b"myos-5g", false);
    // Calibrated for 5 GHz: the LC, then the others for the band.
    let cals: Vec<(u32, u32)> = dev.usb.0.calibrations()[before..].to_vec();
    assert!(cals.ends_with(&[(CAL_LC, 0), (CAL_TX_LOFT, 1), (CAL_TXIQ, 1), (CAL_RXIQC_FI, 1), (CAL_TEMP_SENSOR, 0), (CAL_TX_SHAPING, 0), (CAL_TSSI, 1)]));
    assert_eq!(dev.usb.0.reg(TX_BAND_CFG) & (BAND_2G | BAND_5G), BAND_5G);
    // Its rates: OFDM only, the slowest first.
    assert_eq!(dev.tx_rate(), TxRate::Legacy(4));
    let frame = data(p.bssid);
    dev.send(&frame).unwrap();
    let s = dev.usb.0.sent.last().unwrap();
    assert_eq!((s.endpoint, s.wcid, s.acked, s.rate), (EP_DATA[0], 1, true, PHY_OFDM << RATE_PHY_SHIFT));
    assert_eq!(&s.frame[4..], &frame[4..]);
    // Long slot: 20 µs, the ACK timeout after it.
    assert_eq!(dev.usb.0.reg(BKOFF_SLOT_CFG) & 0xff, 20);

    dev.left().unwrap();
    assert_eq!(dev.usb.0.bssid(), [0; 6]);
    assert_eq!(dev.usb.0.bytes(wcid_addr(1), 6), &[0; 6]);
    dev.stop().unwrap();
    assert!(!dev.usb.0.radio_on);
}

#[test]
fn rate_adaptation_two_streams() {
    // An 802.11n network with a strong signal: AMRR climbs a rate a
    // second from MCS 0, through MCS 7 then 12, to MCS 15, and stays.
    let mut dev = up();
    let p = join(&mut dev, 1, b"myos-open", true);
    assert_eq!(p.ht_mcs, Some(0xffff));
    assert_eq!(dev.tx_rate(), TxRate::Ht(0));
    let rates = traffic(&mut dev, p.bssid, 14);
    let climb: Vec<TxRate> = [1, 2, 3, 4, 5, 6, 7, 12, 13, 14, 15].map(TxRate::Ht).to_vec();
    assert_eq!(&rates[..11], &climb[..]);
    assert!(rates[11..].iter().all(|&r| r == TxRate::Ht(15)));
    let s = dev.usb.0.sent.last().unwrap();
    assert_eq!((s.rate, s.acked, s.txstream), (PHY_HT << RATE_PHY_SHIFT | 15, true, TXSTREAM_E4));
}

#[test]
fn rate_adaptation_weak() {
    // An 802.11g network whose signal carries 9 Mb/s: AMRR stays at it or
    // under, trying 11 now and then.
    let mut dev = up();
    let p = join(&mut dev, 11, b"myos-far", false);
    let rates = traffic(&mut dev, p.bssid, 40);
    assert!(rates.iter().any(|r| r.kbps() == 9000));
    assert!(rates.iter().all(|r| r.kbps() <= 11_000));
    let (fails, ..) = dev.usb.0.tx_counts();
    assert!(fails < 20, "the counters were read: {fails}");
}

#[test]
fn periodic_calibration() {
    let mut dev = up();
    join(&mut dev, 1, b"myos-open", true);
    // Frames from the access point (its beacons, in its station entry)
    // make its signal: strong, so the receive gain goes down.
    for _ in 0..4 {
        hear(&mut dev);
    }
    let before = dev.usb.0.commands.len();
    for _ in 0..3 {
        dev.calibrate().unwrap();
    }
    // The TSSI compensation: triggered, given the chains' slopes and
    // offsets from the EEPROM once measured (with the DPD calibration, the
    // first time), triggered again.
    let comps: Vec<&Vec<u8>> = dev.usb.0.commands[before..]
        .iter()
        .filter(|(c, p)| *c == CMD_CALIBRATION_OP && p[..4] == CAL_TSSI_COMP.to_le_bytes())
        .map(|(_, p)| p)
        .collect();
    assert_eq!(comps.len(), 3);
    assert_eq!((&comps[0][4..8], &comps[2][4..8]), (&[0, 1, 0, 0][..], &[0, 1, 0, 0][..]));
    assert_eq!(comps[1][4..12], [0, 2, 0, 0, 0xc8, 0xc9, 0x10, 0x11]);
    assert_eq!(dev.usb.0.calibrations().iter().filter(|c| c.0 == CAL_DPD).count(), 1);
    // The AGC's gain lowered by 14 for the strong signal.
    let gain = (dev.usb.0.reg(bbp(BBP_AGC, 8)) & AGC_GAIN) >> 8;
    assert_eq!(gain, AGC_GAIN_RESET - 14);
}

#[test]
fn radar_channel() {
    // Tuned for joining on a radar channel: none of the channel's own
    // calibrations, which send.
    let mut dev = up();
    let before = dev.usb.0.calibrations().len();
    let p = Peer { bssid: [2, 0, 0, 0, 0, 52], channel: 52, rates: vec![0x8c], ht_mcs: None, short_preamble: false, short_slot: true };
    assert!(x2::radar(52));
    dev.joining(&p).unwrap();
    assert_eq!(dev.usb.0.calibrations()[before..], [(CAL_RXDCOC, 52)]);
}
