//! The driver (what the `wifi` daemon runs on an MT7601U) against a model
//! of the device (`model/`): its bring-up with the firmware, the channels,
//! a scan of the model's air, joining an access point, sending to it and
//! leaving, with the model's checks on everything it does.

mod model;

use mtw::chip::{self, MT7601U};
use mtw::dev::{Dev, Error, Peer};
use mtw::rate::TxRate;
use mtw::fw::{self, Firmware};
use mtw::reg::{self, MCU_IVB_LEN};
use mtw::rx;
use mtw::usb::{Endpoints, Usb, UsbError};

use model::device::{Device, BULK, EP_DATA, EP_MCU, MAC};

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
    fn bulk_out(&mut self, endpoint: u8, data: &[u8]) -> Result<(), UsbError> {
        self.0.bulk_out(endpoint, data);
        Ok(())
    }
    fn delay_us(&mut self, _us: u32) {}
}

const IVB: [u8; MCU_IVB_LEN] = [0x5a; MCU_IVB_LEN];

/// A firmware file of three blocks and a bit, synthetic.
fn firmware() -> Vec<u8> {
    let ilm: Vec<u8> = (0..20_000u32).map(|i| (i * 7 + 3) as u8).collect();
    fw::build(&IVB, &ilm, &[])
}

/// The radio attached and brought up.
fn up(file: &[u8]) -> Dev<Host> {
    let fw = Firmware::parse(file).unwrap();
    let eps = Endpoints::new(&BULK).unwrap();
    let mut dev = Dev::new(Host(Device::new(fw.ivb, fw.ilm)), &MT7601U, eps).unwrap();
    let info = dev.attach(&fw).unwrap();
    assert_eq!(info.mac, MAC);
    assert_eq!((info.rf_rev, info.ntx, info.nrx, info.mac_ver), (reg::RF_7601, 1, 1, 0x7601));
    assert_eq!(info.fw, Some((fw.build_ver, fw.fw_ver)));
    dev.init(info.mac).unwrap();
    dev
}

/// What the radio hears on the channel it is on now: {frame, dBm}.
fn hear(dev: &mut Dev<Host>) -> Vec<(Vec<u8>, i8)> {
    let ep = dev.rx_endpoint();
    let mut xfer = dev.usb.0.bulk_in(ep);
    let mut heard = Vec::new();
    rx::frames(&mut xfer, |f| heard.push((f.frame.to_vec(), f.rssi)));
    heard.into_iter().map(|(f, r)| (f, dev.rssi_dbm(r))).collect()
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
        ht_mcs: ie(beacon, 45).map(|ht| ht[3]),
        short_preamble: cap & 0x20 != 0,
        short_slot: cap & 0x400 != 0,
    }
}

/// The beacon of the network `name` on `chan`.
fn beacon(dev: &mut Dev<Host>, chan: u8, name: &[u8]) -> Vec<u8> {
    dev.set_chan(chan).unwrap();
    hear(dev).into_iter().map(|(f, _)| f).find(|f| f[0] == 0x80 && ssid(f) == name).expect("its beacon")
}

/// Join network `name` on `chan` (802.11n when `ht`): its peer and the
/// association id.
fn join(dev: &mut Dev<Host>, chan: u8, name: &[u8], ht: bool) -> (Peer, u16) {
    let p = peer(&beacon(dev, chan, name));
    dev.joining(&p).unwrap();
    assert_eq!(dev.usb.0.bssid(), p.bssid);
    dev.send(&mgmt(0xb0, p.bssid, &[0, 0, 1, 0, 0, 0])).unwrap();
    let auth = hear(dev).into_iter().find(|(f, _)| f[0] == 0xb0).expect("authentication answered");
    assert_eq!(&auth.0[24..30], &[0, 0, 2, 0, 0, 0]);
    dev.send(&mgmt(0x00, p.bssid, &[0x01, 0x00, 10, 0, 0, 9])).unwrap();
    let assoc = hear(dev).into_iter().find(|(f, _)| f[0] == 0x10).expect("association answered");
    let aid = u16::from_le_bytes([assoc.0[28], assoc.0[29]]) & 0x3fff;
    dev.associated(&p, aid, ht).unwrap();
    assert_eq!(dev.usb.0.bytes(reg::WCID_ENTRY0 + 8 * u16::from(reg::aid_to_wcid(aid)), 6), &p.bssid);
    (p, aid)
}

/// A data frame from the station to the distribution system through `bssid`.
fn data(bssid: [u8; 6]) -> Vec<u8> {
    let mut f = vec![0x08, 0x01, 0, 0];
    f.extend_from_slice(&bssid);
    f.extend_from_slice(&MAC);
    f.extend_from_slice(&[0xff; 6]);
    f.extend_from_slice(&[0, 0, 0xaa, 0xaa, 3, 0, 0, 0, 0x08, 0x00, 0x45]);
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

fn ssid(beacon: &[u8]) -> &[u8] {
    let ie = &beacon[36..];
    &ie[2..2 + usize::from(ie[1])]
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

#[test]
fn bring_up() {
    let dev = up(&firmware());
    let m = &dev.usb.0;
    assert!(m.running && m.radio_on);
    assert_eq!(m.mac_addr(), MAC);
    assert_eq!(m.tuned, Some(1));
    // A station's receive filter, the MAC and DMA on, aggregation.
    assert_ne!(m.reg(reg::RX_FILTR_CFG) & reg::DROP_UC_NOME, 0);
    assert_eq!(m.reg(reg::MAC_SYS_CTRL), reg::MAC_RX_EN | reg::MAC_TX_EN);
    assert_ne!(m.reg(reg::USB_DMA_CFG) & reg::USB_RX_AGG_EN, 0);
    // The baseband's defaults, then 20 MHz and the AGC on top.
    assert_eq!((m.bbp(1), m.bbp(66)), (0x04, 0x14));
    assert_eq!(m.bbp(4) & 0x58, 0x40);
    // The MCU's calibrations, in mt7601_rf_setup's order, then the
    // channel group's.
    let cals: Vec<u32> = m
        .commands
        .iter()
        .filter(|(c, _)| *c == reg::MCU_CMD_CALIBRATE)
        .map(|(_, p)| u32::from_le_bytes(p[..4].try_into().unwrap()))
        .collect();
    assert_eq!(cals, [1, 9, 6, 6, 4, 5, 8, 6]);
}

#[test]
fn firmware_already_running() {
    let file = firmware();
    let dev = up(&file);
    // The daemon again, the device not reset: the firmware is not sent twice.
    let fw = Firmware::parse(&file).unwrap();
    let mut again = Dev::new(dev.usb, &MT7601U, Endpoints::new(&BULK).unwrap()).unwrap();
    assert_eq!(again.attach(&fw).unwrap().fw, None);
    again.init(MAC).unwrap();
}

#[test]
fn other_chip() {
    let file = firmware();
    let fw = Firmware::parse(&file).unwrap();
    let mut model = Device::new(fw.ivb, fw.ilm);
    model.set_asic(0x7610_0001);
    let r = Dev::new(Host(model), &MT7601U, Endpoints::new(&BULK).unwrap());
    assert_eq!(r.err(), Some(Error::Chip(0x7610)));
    assert_eq!(chip::Chip::by_id((0x148f, 0x7601)), Some(&MT7601U));
}

#[test]
fn channels_and_scan() {
    let mut dev = up(&firmware());
    assert_eq!(dev.set_chan(15).err(), Some(Error::Channel(15)));
    for (chan, name, dbm) in [(1, &b"myos-open"[..], -40), (6, b"myos-wpa2", -55), (11, b"myos-far", -82)] {
        dev.set_chan(chan).unwrap();
        assert_eq!(dev.usb.0.tuned, Some(chan));
        // A wildcard probe request goes out at 1 Mb/s, wanting no ACK.
        let mut probe = mgmt(0x40, [0xff; 6], &[0, 0, 1, 4, 0x02, 0x04, 0x0b, 0x16]);
        probe[16..22].copy_from_slice(&[0xff; 6]);
        dev.send(&probe).unwrap();
        let s = dev.usb.0.sent.last().unwrap();
        assert_eq!((s.channel, s.endpoint, s.phy, s.xflags & reg::TX_ACK), (chan, EP_MCU, 0, 0));
        let heard = hear(&mut dev);
        let beacons: Vec<_> = heard.iter().filter(|(f, _)| f[0] == 0x80).collect();
        assert_eq!(beacons.len(), 1, "channel {chan}");
        assert_eq!((ssid(&beacons[0].0), beacons[0].1), (name, dbm));
    }
}

#[test]
fn join_send_leave() {
    let mut dev = up(&firmware());
    let (p, aid) = join(&mut dev, 1, b"myos-open", false);
    // A data frame to the access point: the slowest of its rates to start
    // with, ACKed, its entry.
    let frame = data(p.bssid);
    dev.send(&frame).unwrap();
    let s = dev.usb.0.sent.last().unwrap();
    assert_eq!(s.phy, 0);
    assert_eq!((s.endpoint, s.wcid, s.acked), (EP_DATA[0], reg::aid_to_wcid(aid), true));
    assert_eq!(&s.frame[4..], &frame[4..]);
    // Its basic rates are the MAC's, its preamble and slot the long ones.
    assert_eq!(dev.usb.0.reg(reg::LEGACY_BASIC_RATE), 0xf);
    assert_eq!(dev.usb.0.reg(reg::BKOFF_SLOT_CFG) & 0xff, 20);

    dev.left().unwrap();
    assert_eq!(dev.usb.0.bssid(), [0; 6]);
    assert_eq!(dev.tx_rate(), TxRate::Legacy(reg::RIDX_CCK1));
    dev.stop().unwrap();
    assert!(!dev.usb.0.radio_on);
}

#[test]
fn rate_adaptation_ht() {
    // An 802.11n network with a strong signal: AMRR climbs a MCS a second
    // from MCS 0, to MCS 7, and stays.
    let mut dev = up(&firmware());
    let (p, _) = join(&mut dev, 1, b"myos-open", true);
    assert_eq!(p.ht_mcs, Some(0xff));
    assert_eq!(dev.tx_rate(), TxRate::Ht(0));
    let rates = traffic(&mut dev, p.bssid, 10);
    assert_eq!(&rates[..7], &(1..8).map(TxRate::Ht).collect::<Vec<_>>()[..]);
    assert!(rates[7..].iter().all(|&r| r == TxRate::Ht(7)));
    let s = dev.usb.0.sent.last().unwrap();
    assert_eq!((s.phy, s.acked), (reg::PHY_HT << reg::PHY_SHIFT | 7, true));
}

#[test]
fn rate_adaptation_weak() {
    // An 802.11g network whose signal carries 9 Mb/s: AMRR climbs to it,
    // tries 11 Mb/s now and then, and is back at 9 or under after each.
    let mut dev = up(&firmware());
    let (p, _) = join(&mut dev, 11, b"myos-far", false);
    let rates = traffic(&mut dev, p.bssid, 40);
    let kbps = |r: &TxRate| match r {
        TxRate::Legacy(i) => u32::from(reg::RATES[*i].rate) * 500,
        TxRate::Ht(_) => panic!("an 802.11g network"),
    };
    assert!(rates.iter().any(|r| kbps(r) == 9000));
    assert!(rates.iter().all(|r| kbps(r) <= 11_000));
    // A failure after an increase doubles the wait before the next: the
    // tries at 11 Mb/s get rarer.
    let tries: Vec<usize> = (0..rates.len()).filter(|&i| kbps(&rates[i]) == 11_000).collect();
    assert!(tries.len() >= 2 && tries.len() <= 8, "{tries:?}");
    let (fails, ..) = dev.usb.0.tx_counts();
    assert!(fails < 20, "the counters were read: {fails}");
}
