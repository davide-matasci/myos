//! Intel's Wi-Fi 6 radios (issue #495): the AX201 and the AX200, each as
//! `iwx::chip` describes it, with the `iwx` crate's driver over
//! `/dev/pci` and its firmware from `wifi-firmware`. The firmware scans by
//! itself (Radio::scan) and keeps the network joined (Radio::joining,
//! associated, left); the daemon's frames go out as they are, encrypted by
//! the daemon on a WPA2 network.

use alloc::vec::Vec;

use iwx::chip::{CHIPS, Chip};
use iwx::dev::{Dev, Error, Info, Peer};
use iwx::fw::Firmware;
use iwx::msg::{self, Channel};
use iwx::sta::{self, Beacon};

use crate::devs::load;
use crate::frame::Bss;
use crate::pci::{self, Pci};
use crate::radio::{Heard, Radio, Rx};

/// Where `wifi-firmware fetch` keeps the firmware by default.
const FIRMWARE_DIRS: &[&[u8]] = &[b"/data/firmware/", b"/tmp/firmware/"];
/// A scan of every channel takes a few seconds (110 ms per passive one).
const SCAN_TIMEOUT_US: u64 = 15_000_000;

pub struct Intel {
    dev: Dev<Pci>,
    fw: Firmware<'static>,
    info: Info,
    irq: usize,
    /// The last beacon timing heard from each access point: the firmware
    /// needs it of the one joined.
    beacons: Vec<([u8; 6], Beacon)>,
    /// The network being joined, or joined.
    peer: Option<Peer>,
}

/// Why the radio did not come up, for the daemon to say.
pub enum Failed {
    Pci(&'static str),
    NoFirmware,
    BadFirmware,
    Driver(Error),
}

impl Intel {
    /// The radio `chip` at `/dev/pci` directory `dir`: held, its firmware
    /// loaded and started, configured up to a scan.
    pub fn open(dir: &[u8], chip: &'static Chip) -> Result<Self, Failed> {
        let name = chip.firmware.as_bytes();
        let file = FIRMWARE_DIRS.iter().find_map(|d| load(&[d, name].concat())).ok_or(Failed::NoFirmware)?;
        // The daemon's for good: the firmware (its paging part) is the
        // device's as long as it runs.
        let file: &'static [u8] = file.leak();
        let fw = Firmware::parse(file).map_err(|_| Failed::BadFirmware)?;
        let pci = Pci::open(dir).map_err(Failed::Pci)?;
        let (msix, irq) = (pci.msix, pci.irq_fd());
        let mut dev = Dev::new(pci, msix, chip).map_err(Failed::Driver)?;
        dev.load(&fw).map_err(Failed::Driver)?;
        let info = dev.init(&fw).map_err(Failed::Driver)?;
        Ok(Intel { dev, fw, info, irq, beacons: Vec::new(), peer: None })
    }

    /// Frame `m` heard: kept as its access point's latest beacon when it
    /// is one.
    fn heard(&mut self, m: &msg::Mpdu) {
        let Some((bssid, b)) = sta::beacon(&m.frame, m.gp2) else {
            return;
        };
        match self.beacons.iter_mut().find(|(a, _)| *a == bssid) {
            Some(e) => e.1 = b,
            None => self.beacons.push((bssid, b)),
        }
    }
}

/// The first of the radios in `/dev/pci`, if there is one: its directory
/// and which it is.
pub fn find() -> Option<(Vec<u8>, &'static Chip)> {
    let ids: Vec<(u16, u16)> = CHIPS.iter().flat_map(|c| c.ids.iter().copied()).collect();
    let (dir, id) = pci::find(&ids)?;
    Some((dir, Chip::by_id(id)?))
}

impl Radio for Intel {
    fn mac(&self) -> [u8; 6] {
        self.info.mac
    }

    /// The firmware tunes itself: to scan, and to the network joined.
    fn set_channel(&mut self, _channel: u8) {}

    /// On the network being joined only: the firmware sends nothing else.
    fn send(&mut self, frame: &[u8]) -> bool {
        let Some(p) = self.peer else {
            return false;
        };
        self.dev.send(frame, p.channel.ghz5).is_ok()
    }

    fn recv(&mut self, buf: &mut [u8]) -> Option<Rx> {
        while let Some(n) = self.dev.poll() {
            if n.code != msg::REPLY_RX_MPDU {
                continue;
            }
            let Some(m) = msg::mpdu(&n.data) else {
                continue;
            };
            self.heard(&m);
            let len = m.frame.len().min(buf.len());
            buf[..len].copy_from_slice(&m.frame[..len]);
            return Some(Rx { len, channel: m.channel, rssi: m.dbm.clamp(-128, 0) as i8 });
        }
        None
    }

    fn fd(&self) -> usize {
        self.irq
    }

    fn pending(&self) -> bool {
        self.dev.pending()
    }

    /// The firmware set up for `bss` (its channel as the regulatory
    /// domain has it, its beacon timing from the scan) and kept on its
    /// channel while the daemon authenticates and associates.
    fn joining(&mut self, bss: &Bss) -> bool {
        self.left();
        let channel = self.info.channels.iter().copied().find(|c| c.number == bss.channel).unwrap_or(Channel {
            number: bss.channel,
            ghz5: bss.channel > 14,
            passive: false,
        });
        let beacon = self.beacons.iter().find(|(a, _)| *a == bss.bssid).map_or(
            Beacon { tsf: 0, gp2: 0, interval: 100, dtim_period: 0, dtim_count: 0, rates: 0 },
            |e| e.1,
        );
        let peer = Peer { bssid: bss.bssid, channel, beacon };
        if self.dev.join(&self.info, &peer).is_err() {
            return false;
        }
        self.peer = Some(peer);
        true
    }

    fn associated(&mut self, aid: u16, _ht: bool) -> bool {
        let Some(mut p) = self.peer else {
            return false;
        };
        // The latest beacon, for the DTIM timing.
        if let Some(e) = self.beacons.iter().find(|(a, _)| *a == p.bssid) {
            p.beacon = e.1;
        }
        self.dev.associated(&self.info, &p, aid).is_ok()
    }

    /// What was sent goes first (the deauthentication), then the firmware
    /// lets the network go.
    fn left(&mut self) {
        if let Some(p) = self.peer.take() {
            self.dev.drain(200_000);
            self.dev.leave(&self.info, &p);
        }
    }

    /// The network let go of first, then the firmware halted and the
    /// device reset: a reboot keeps an integrated radio powered, and
    /// Windows' driver took one left running for a broken device.
    fn stop(&mut self) {
        self.left();
        self.dev.stop();
    }

    /// The firmware's passive scan of every channel the regulatory domain
    /// allows: the frames heard until it says it is done.
    fn scan(&mut self) -> Option<Vec<Heard>> {
        // What came since the last scan is stale.
        while self.dev.poll().is_some() {}
        if self.dev.scan(&self.fw, &self.info).is_err() {
            return Some(Vec::new());
        }
        let mut heard = Vec::new();
        let end = myos_user::monotonic_ns() / 1000 + SCAN_TIMEOUT_US;
        loop {
            let left = end.saturating_sub(myos_user::monotonic_ns() / 1000);
            let Ok(Some(n)) = self.dev.next(left) else {
                break;
            };
            match n.code {
                msg::REPLY_RX_MPDU => {
                    if let Some(m) = msg::mpdu(&n.data) {
                        self.heard(&m);
                        heard.push(Heard { frame: m.frame, channel: m.channel, rssi: m.dbm.clamp(-128, 0) as i8 });
                    }
                }
                msg::SCAN_COMPLETE_UMAC => break,
                _ => {}
            }
        }
        Some(heard)
    }
}

/// What to tell the user when the radio `chip` did not come up.
pub fn why(chip: &Chip, f: &Failed) -> Vec<u8> {
    let mut s = alloc::format!("wifi: {}: ", chip.name).into_bytes();
    match f {
        Failed::Pci(what) => s.extend_from_slice(what.as_bytes()),
        Failed::NoFirmware => s.extend_from_slice(alloc::format!("no {} (wifi-firmware fetch)", chip.firmware).as_bytes()),
        Failed::BadFirmware => s.extend_from_slice(b"its firmware file is not one (wifi-firmware fetch)"),
        Failed::Driver(e) => s.extend_from_slice(alloc::format!("{e:?}").as_bytes()),
    }
    s.push(b'\n');
    s
}
