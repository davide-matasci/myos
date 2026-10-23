//! The driver (what the `wifi` daemon runs on the Raspberry Pi's Wi-Fi
//! chip) against a model of the chip (`model/`): the bring-up with the
//! firmware and NVRAM, the firmware's set-up, a scan, joining an open and
//! a WPA2-PSK network, frames both ways, leaving, and the model's checks
//! on everything it is told.

mod model;

use bwfm::chip::{CC_4345_CHIP_ID, CC_43430_CHIP_ID, Known};
use bwfm::dev::{Dev, Link};
use bwfm::event::Scanned;
use bwfm::fw;
use bwfm::reg::*;
use bwfm::Error;

use model::device::{Device, Kind, MAC, RAMSIZE};

/// A firmware image: a reset vector, then a pattern.
fn firmware() -> Vec<u8> {
    let mut f = Vec::from(0x0019_8100u32.to_le_bytes());
    f.extend((0..6000u32).map(|i| (i * 13 + 7) as u8));
    f
}

const NVRAM: &[u8] = b"# Cloud9 Wi-Fi NVRAM\nmanfid=0x2d0\nprodid=0x0727\nvendid=0x14e4\n\nboardrev=0x1103\n";

fn clm() -> Vec<u8> {
    (0..3300u32).map(|i| (i * 7) as u8).collect()
}

/// The chip attached and its firmware started.
fn up(kind: Kind) -> Dev<Device> {
    let mut dev = Dev::attach(Device::new(kind)).unwrap();
    assert!(!dev.bus.sdio.running, "the CPU is held before the firmware is loaded");
    let info = dev.start(&firmware(), NVRAM, Some(&clm())).unwrap();
    assert_eq!((info.mac, info.io_type), (MAC, IO_TYPE_D11AC));
    assert!(dev.bus.sdio.running && dev.bus.sdio.kind == kind);
    dev
}

/// Serve the card until `done`, a bounded number of times.
fn until<S: FnMut(&mut Dev<Device>) -> bool>(dev: &mut Dev<Device>, mut done: S) {
    for _ in 0..50 {
        dev.service().unwrap();
        if done(dev) {
            return;
        }
    }
    panic!("not reached");
}

#[test]
fn attaches_to_the_chips() {
    let dev = Dev::attach(Device::new(Kind::Cyw43455)).unwrap();
    let chip = &dev.bus.chip;
    assert_eq!((chip.id, chip.rev), (CC_4345_CHIP_ID, 6));
    assert_eq!((chip.rambase, chip.ramsize), (0x19_8000, RAMSIZE));
    assert!(chip.core(CORE_ARM_CR4).is_some() && chip.core(CORE_SDIO_DEV).is_some());
    assert_eq!(dev.known().unwrap().name, "CYW43455");
    assert_eq!(dev.bus.sdio.block, [0, 64, 512, 0, 0, 0, 0, 0]);
    let dev = Dev::attach(Device::new(Kind::Cyw43438)).unwrap();
    let chip = &dev.bus.chip;
    assert_eq!((chip.id, chip.rambase, chip.ramsize, chip.srsize), (CC_43430_CHIP_ID, 0, RAMSIZE, 64 * 1024));
    assert!(chip.core(CORE_ARM_CM3).is_some() && chip.core(CORE_INTERNAL_MEM).is_some());
    assert_eq!(Known::by_id(chip.id).unwrap().firmware, "cyfmac43430-sdio");
}

#[test]
fn starts_the_firmware() {
    for kind in [Kind::Cyw43455, Kind::Cyw43438] {
        let dev = up(kind);
        let m = &dev.bus.sdio;
        // The image at the RAM's start, the NVRAM at its end, as the
        // firmware found them.
        let ram = m.booted_with.as_ref().unwrap();
        assert_eq!(&ram[..6004], &firmware()[..]);
        let nvram = fw::nvram(NVRAM, None);
        let at = RAMSIZE as usize - nvram.len();
        assert_eq!(&ram[at..], &nvram[..]);
        // What the firmware was told.
        assert_eq!(m.vars["sup_wpa"], 1u32.to_le_bytes());
        assert_eq!(m.vars["mpc"], 1u32.to_le_bytes());
        let mask = &m.vars["event_msgs"];
        assert!(mask[E_ESCAN_RESULT as usize / 8] & (1 << (E_ESCAN_RESULT % 8)) != 0);
        assert!(mask[E_LINK as usize / 8] & (1 << (E_LINK % 8)) != 0);
        assert_eq!(m.clm, clm());
        assert!(m.cmds.iter().any(|(c, d)| *c == C_SET_PM && d[..4] == PM_FAST_PS.to_le_bytes()));
        assert!(m.cmds.iter().any(|(c, _)| *c == C_UP));
        assert_eq!(dev.bus.console_addr, dev.bus.chip.rambase + RAMSIZE - 128);
    }
}

#[test]
fn a_firmware_that_does_not_fit_is_refused() {
    let mut dev = Dev::attach(Device::new(Kind::Cyw43455)).unwrap();
    let big = vec![0u8; RAMSIZE as usize];
    assert_eq!(dev.start(&big, NVRAM, None).unwrap_err(), Error::Firmware("firmware and NVRAM do not fit the RAM"));
}

fn scan(dev: &mut Dev<Device>) -> Vec<Scanned> {
    dev.scan().unwrap();
    assert!(dev.scanning());
    let mut found = None;
    until(dev, |d| {
        found = d.scan_results();
        found.is_some()
    });
    found.unwrap()
}

#[test]
fn scans_the_air() {
    let mut dev = up(Kind::Cyw43455);
    let found = scan(&mut dev);
    assert_eq!(found.len(), 2);
    let open = found.iter().find(|s| s.ssid == b"myos-open").unwrap();
    assert_eq!((open.bssid, open.channel, open.rssi, open.capability), ([0x02, 0x00, 0x5e, 0x00, 0x01, 0x01], 1, -40, 0x0401));
    assert_eq!(open.ies, [vec![0, 9], b"myos-open".to_vec()].concat());
    let psk = found.iter().find(|s| s.ssid == b"myos-psk").unwrap();
    assert_eq!((psk.channel, psk.rssi), (6, -55));
    assert_eq!(psk.ies[10], 0x30, "the RSN element");
    assert!(!dev.scanning());
    assert!(dev.scan_results().is_none(), "taken once");
}

#[test]
fn joins_an_open_network_and_carries_frames() {
    let mut dev = up(Kind::Cyw43455);
    let found = scan(&mut dev);
    let open = found.iter().find(|s| s.ssid == b"myos-open").unwrap().clone();
    dev.join(&open.ssid, open.bssid, open.channel, None).unwrap();
    assert_eq!(dev.link(), Link::Joining);
    until(&mut dev, |d| d.link() == Link::Up);
    assert_eq!(dev.bssid(), Some(open.bssid));
    let m = &dev.bus.sdio;
    assert_eq!(m.vars["wpa_auth"], WPA_AUTH_DISABLED.to_le_bytes());
    assert_eq!(m.vars["wsec"], WSEC_NONE.to_le_bytes());
    assert_eq!(m.joined, Some(open.bssid));
    // Frames out, with a BCDC header the model stripped.
    let mut eth = Vec::new();
    eth.extend_from_slice(&open.bssid);
    eth.extend_from_slice(&MAC);
    eth.extend_from_slice(&[0x08, 0x00]);
    eth.extend_from_slice(&[1u8; 60]);
    dev.send(&eth).unwrap();
    assert_eq!(dev.bus.sdio.sent, vec![eth.clone()]);
    // Frames in.
    let mut back = eth.clone();
    back[..6].copy_from_slice(&MAC);
    back[6..12].copy_from_slice(&open.bssid);
    dev.bus.sdio.inject(&back);
    assert!(!dev.pending());
    dev.service().unwrap();
    assert!(dev.pending());
    assert_eq!(dev.recv().unwrap(), back);
    assert!(dev.recv().is_none());
    // The access point lets us go.
    dev.bus.sdio.disconnect();
    until(&mut dev, |d| d.link() == Link::Down);
    // Frames while down are dropped.
    dev.bus.sdio.inject(&back);
    dev.service().unwrap();
    assert!(dev.recv().is_none());
}

#[test]
fn joins_a_wpa2_network_with_the_firmware_supplicant() {
    let mut dev = up(Kind::Cyw43438);
    let found = scan(&mut dev);
    let psk = found.iter().find(|s| s.ssid == b"myos-psk").unwrap().clone();
    let pmk = [0x5a; 32];
    dev.join(&psk.ssid, psk.bssid, psk.channel, Some(&pmk)).unwrap();
    until(&mut dev, |d| d.link() == Link::Up);
    let m = &dev.bus.sdio;
    assert_eq!(m.vars["wpa_auth"], WPA_AUTH_WPA2_PSK.to_le_bytes());
    assert_eq!(m.vars["wsec"], WSEC_AES.to_le_bytes());
    assert_eq!(m.vars["auth"], AUTH_OPEN.to_le_bytes());
    assert_eq!(m.pmk_hex.as_deref(), Some(&b"5a".repeat(32)[..]));
    // Leaving.
    dev.leave().unwrap();
    assert_eq!(dev.link(), Link::Down);
    assert!(dev.bus.sdio.cmds.iter().any(|(c, _)| *c == C_DISASSOC));
    assert_eq!(dev.bus.sdio.joined, None);
    // The wrong key: the access point throws us out before the keys are
    // in place, which the driver reports as the link lost while joining.
    let wrong = [0x11; 32];
    dev.join(&psk.ssid, psk.bssid, psk.channel, Some(&wrong)).unwrap();
    for _ in 0..10 {
        dev.service().unwrap();
    }
    assert_ne!(dev.link(), Link::Up);
}

#[test]
fn a_network_that_is_not_there() {
    let mut dev = up(Kind::Cyw43455);
    dev.join(b"nowhere", [9; 6], 1, None).unwrap();
    until(&mut dev, |d| matches!(d.link(), Link::Failed(_)));
    assert_eq!(dev.link(), Link::Failed(E_STATUS_NO_NETWORKS));
    assert_eq!(dev.join(&[0u8; 33], [9; 6], 1, None).unwrap_err(), Error::Protocol("SSID length"));
}
