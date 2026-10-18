//! The AX201's radio (Intel Wi-Fi 6, 8086:06f0, issue #495): the `iwx`
//! crate's driver over `/dev/pci`, its firmware from `wifi-firmware`. The
//! firmware scans by itself (Radio::scan); sending frames, and so joining
//! a network, is not there yet (issue #495).

use alloc::vec::Vec;

use iwx::dev::{Dev, Error, Info};
use iwx::fw::Firmware;
use iwx::msg;
use myos_user::{close, open_flags, read, O_RDONLY};

use crate::pci::{self, Pci};
use crate::radio::{Heard, Radio, Rx};

/// The PCI ids driven: the AX201s with a QuZ MAC (Comet Lake H, the
/// ThinkPad T15p's, and Comet Lake LP), as OpenBSD's iwx.
pub const IDS: &[(u16, u16)] = &[(0x8086, 0x06f0), (0x8086, 0x02f0)];
const FIRMWARE: &[u8] = b"iwlwifi-QuZ-a0-hr-b0-77.ucode";
/// Where `wifi-firmware fetch` keeps it by default.
const FIRMWARE_DIRS: &[&[u8]] = &[b"/data/firmware/", b"/tmp/firmware/"];
/// A scan of every channel takes a few seconds (110 ms per passive one).
const SCAN_TIMEOUT_US: u64 = 15_000_000;

pub struct Ax201 {
    dev: Dev<Pci>,
    fw: Firmware<'static>,
    info: Info,
    irq: usize,
}

/// Why the radio did not come up, for the daemon to say.
pub enum Failed {
    Pci(&'static str),
    NoFirmware,
    BadFirmware,
    Driver(Error),
}

impl Ax201 {
    /// The radio at `/dev/pci` directory `dir`: held, its firmware loaded
    /// and started, configured up to a scan.
    pub fn open(dir: &[u8]) -> Result<Self, Failed> {
        let file = FIRMWARE_DIRS.iter().find_map(|d| load(&[d, FIRMWARE].concat())).ok_or(Failed::NoFirmware)?;
        // The daemon's for good: the firmware (its paging part) is the
        // device's as long as it runs.
        let file: &'static [u8] = file.leak();
        let fw = Firmware::parse(file).map_err(|_| Failed::BadFirmware)?;
        let pci = Pci::open(dir).map_err(Failed::Pci)?;
        let (msix, irq) = (pci.msix, pci.irq_fd());
        let mut dev = Dev::new(pci, msix).map_err(Failed::Driver)?;
        dev.load(&fw).map_err(Failed::Driver)?;
        let info = dev.init(&fw).map_err(Failed::Driver)?;
        Ok(Ax201 { dev, fw, info, irq })
    }
}

/// The first AX201 in `/dev/pci`, if there is one.
pub fn find() -> Option<Vec<u8>> {
    pci::find(IDS).map(|(dir, _)| dir)
}

fn load(path: &[u8]) -> Option<Vec<u8>> {
    let fd = open_flags(path, O_RDONLY)?;
    let mut data = Vec::new();
    let mut buf = [0u8; 16384];
    loop {
        let n = read(fd, &mut buf);
        if n == usize::MAX {
            close(fd);
            return None;
        }
        if n == 0 {
            break;
        }
        data.extend_from_slice(&buf[..n]);
    }
    close(fd);
    Some(data)
}

impl Radio for Ax201 {
    fn mac(&self) -> [u8; 6] {
        self.info.mac
    }

    /// The firmware tunes itself (for its scans so far).
    fn set_channel(&mut self, _channel: u8) {}

    fn send(&mut self, _frame: &[u8]) -> bool {
        false
    }

    fn recv(&mut self, buf: &mut [u8]) -> Option<Rx> {
        while let Ok(Some(n)) = self.dev.next(0) {
            if n.code != msg::REPLY_RX_MPDU {
                continue;
            }
            let Some(m) = msg::mpdu(&n.data) else {
                continue;
            };
            let len = m.frame.len().min(buf.len());
            buf[..len].copy_from_slice(&m.frame[..len]);
            return Some(Rx { len, channel: m.channel, rssi: m.dbm.clamp(-128, 0) as i8 });
        }
        None
    }

    fn fd(&self) -> usize {
        self.irq
    }

    fn joins(&self) -> bool {
        false
    }

    /// The firmware's passive scan of every channel the regulatory domain
    /// allows: the frames heard until it says it is done.
    fn scan(&mut self) -> Option<Vec<Heard>> {
        // What came since the last scan is stale.
        while let Ok(Some(_)) = self.dev.next(0) {}
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

/// What to tell the user when the AX201 did not come up.
pub fn why(f: &Failed) -> Vec<u8> {
    let mut s = Vec::from(b"wifi: AX201: " as &[u8]);
    match f {
        Failed::Pci(what) => s.extend_from_slice(what.as_bytes()),
        Failed::NoFirmware => s.extend_from_slice(b"no firmware (wifi-firmware fetch)"),
        Failed::BadFirmware => s.extend_from_slice(b"its firmware file is not one (wifi-firmware fetch)"),
        Failed::Driver(e) => s.extend_from_slice(alloc::format!("{e:?}").as_bytes()),
    }
    s.push(b'\n');
    s
}
