//! Broadcom's (now Infineon's, Cypress's before) SDIO radios: the
//! Raspberry Pi's CYW43455 (3 B+, 3 A+, 4, 400) and CYW43438 (3 B, Zero
//! W), as `bwfm::chip` knows them, with the `bwfm` crate's driver over
//! `/dev/sdio` and their firmware, NVRAM and CLM from `wifi-firmware`. A
//! FullMAC: the firmware scans, joins, keeps the keys (its own WPA2
//! supplicant) and carries Ethernet frames; the daemon tells it what to
//! do and waits for its events.

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use bwfm::chip::{Known, CHIPS, MANUFACTURER};
use bwfm::dev::{Dev, Info, Link};
use bwfm::Error;
use myos_user::{monotonic_ns, poll, sleep_ns, PollFd, POLLIN};

use crate::devs::{self, load, slurp, value};
use crate::frame::{self, Bss};
use crate::radio::{FullMac, Seen};
use crate::sdio::Sdio;

/// Where `wifi-firmware fetch` keeps the firmware by default.
const FIRMWARE_DIRS: &[&[u8]] = &[b"/data/firmware/", b"/tmp/firmware/"];
/// How long a scan may take (every channel, 2.4 and 5 GHz).
const SCAN_TIMEOUT_MS: u64 = 15_000;
/// How long joining may take, the firmware's 4-way handshake included.
const JOIN_TIMEOUT_MS: u64 = 12_000;

/// The boards and their NVRAM (`brcmfmac4xxxx-sdio.BOARD.txt` in
/// linux-firmware), by the start of the tree's `model`; the first match
/// wins. The 3 A+ and the Zero W have the 3 B+'s and the 3 B's
/// (linux-firmware links theirs to them).
const BOARDS: &[(&str, &str)] = &[
    ("Raspberry Pi 3 Model B Plus", "raspberrypi,3-model-b-plus"),
    ("Raspberry Pi 3 Model A Plus", "raspberrypi,3-model-b-plus"),
    ("Raspberry Pi 3 Model B", "raspberrypi,3-model-b"),
    ("Raspberry Pi Zero W", "raspberrypi,3-model-b"),
    ("Raspberry Pi 4 Model B", "raspberrypi,4-model-b"),
    ("Raspberry Pi 400", "raspberrypi,4-model-b"),
    ("Raspberry Pi Compute Module 4", "raspberrypi,4-model-b"),
];

pub struct Broadcom {
    dev: Dev<Sdio>,
    info: Info,
}

/// Why the radio did not come up, for the daemon to say.
pub enum Failed {
    Sdio(&'static str),
    /// The board (`/proc/platform`'s model) has no NVRAM here.
    NoBoard(Vec<u8>),
    /// A firmware file is missing (its name).
    NoFirmware(String),
    Driver(Error),
}

impl Broadcom {
    /// The radio `chip` at `/dev/sdio` directory `dir`: held, its
    /// firmware, the board's NVRAM and the CLM loaded, the firmware
    /// started and set up.
    pub fn open(dir: &[u8], chip: &'static Known) -> Result<Self, Failed> {
        let model = slurp(b"/proc/platform").and_then(|t| value(&t, b"model ").map(|m| m.to_vec())).unwrap_or_default();
        let model = model.strip_suffix(b" (dt)").unwrap_or(&model);
        let board = BOARDS.iter().find(|(m, _)| model.starts_with(m.as_bytes())).map(|(_, b)| *b).ok_or_else(|| Failed::NoBoard(model.to_vec()))?;
        let firmware = find(&format!("{}.bin", chip.firmware))?;
        let nvram = find(&format!("{}.{}.txt", chip.nvram, board))?;
        // The CLM: a firmware whose own is enough comes up without.
        let clm = find(&format!("{}.clm_blob", chip.firmware)).ok();
        let sdio = Sdio::open(dir).map_err(Failed::Sdio)?;
        let mut dev = Dev::attach(sdio).map_err(Failed::Driver)?;
        if dev.known().map(|k| k.id) != Some(chip.id) {
            return Err(Failed::Driver(Error::Chip("not the chip its SDIO id says")));
        }
        let info = dev.start(&firmware, &nvram, clm.as_deref()).map_err(Failed::Driver)?;
        // The card's interrupt wakes the daemon, when the slot has a line;
        // else it looks now and then (the daemon's LOOK_MS).
        dev.bus.sdio.irq_on();
        Ok(Broadcom { dev, info })
    }

    /// Wait up to `ms` for the card's interrupt (or `ms` when it has no
    /// line), then serve the card.
    fn wait(&mut self, ms: i32) -> Result<(), Error> {
        match self.dev.bus.sdio.irq {
            Some(fd) => {
                let mut fds = [PollFd { fd: fd as i32, events: POLLIN, revents: 0 }];
                poll(&mut fds, ms);
            }
            None => sleep_ns(u64::from(ms.max(1) as u32) * 1_000_000, false),
        }
        self.serve()
    }

    /// Serve the card: the interrupt taken (if one came), the firmware's
    /// frames and events handled, the interrupt let through again.
    fn serve(&mut self) -> Result<(), Error> {
        self.dev.bus.sdio.irq_take();
        let r = self.dev.service();
        self.dev.bus.sdio.irq_rearm();
        r
    }
}

/// The first of the radios in `/dev/sdio`, if there is one: its directory
/// and which it is.
pub fn find_radio() -> Option<(Vec<u8>, &'static Known)> {
    let ids: Vec<(u16, u16)> = CHIPS.iter().map(|c| (MANUFACTURER, c.product)).collect();
    let (dir, (_, product)) = devs::find(b"/dev/sdio/", &ids)?;
    Some((dir, Known::by_product(product)?))
}

/// The firmware file `name` from the firmware directories.
fn find(name: &str) -> Result<Vec<u8>, Failed> {
    FIRMWARE_DIRS.iter().find_map(|d| load(&[d, name.as_bytes()].concat())).ok_or_else(|| Failed::NoFirmware(String::from(name)))
}

fn now_ms() -> u64 {
    monotonic_ns() / 1_000_000
}

impl FullMac for Broadcom {
    fn mac(&self) -> [u8; 6] {
        self.info.mac
    }

    fn scan(&mut self) -> Result<Vec<Seen>, String> {
        self.dev.scan().map_err(|e| format!("the scan: {e:?}"))?;
        let end = now_ms() + SCAN_TIMEOUT_MS;
        let found = loop {
            self.wait(100).map_err(|e| format!("the scan: {e:?}"))?;
            if let Some(found) = self.dev.scan_results() {
                break found;
            }
            if now_ms() > end {
                return Err(String::from("the scan did not end"));
            }
        };
        let mut seen: Vec<Seen> = found
            .into_iter()
            .map(|s| Seen { bss: frame::bss_of(s.bssid, s.capability, &s.ies, s.channel), rssi: s.rssi.clamp(-128, 127) as i8 })
            .collect();
        seen.sort_by(|a, b| b.rssi.cmp(&a.rssi));
        Ok(seen)
    }

    fn join(&mut self, bss: &Bss, pmk: Option<&[u8; 32]>) -> Result<(), String> {
        self.dev.join(&bss.ssid, bss.bssid, bss.channel, pmk).map_err(|e| format!("the firmware refused the join: {e:?}"))?;
        let end = now_ms() + JOIN_TIMEOUT_MS;
        loop {
            self.wait(100).map_err(|e| format!("joining: {e:?}"))?;
            match self.dev.link() {
                Link::Up => return Ok(()),
                Link::Joining if now_ms() <= end => {}
                Link::Joining => {
                    let _ = self.dev.leave();
                    return Err(String::from("no answer from the network"));
                }
                Link::Failed(status) => return Err(format!("the firmware could not join: status {status}")),
                Link::Lost(15) if pmk.is_some() => return Err(String::from("the 4-way handshake did not finish: wrong passphrase?")),
                Link::Lost(reason) => return Err(format!("the access point let the station go, reason {reason}")),
                Link::Down => return Err(String::from("the link did not come up")),
            }
        }
    }

    fn leave(&mut self) {
        let _ = self.dev.leave();
    }

    fn up(&self) -> bool {
        self.dev.link() == Link::Up
    }

    fn service(&mut self) {
        let _ = self.serve();
    }

    fn send(&mut self, eth: &[u8]) -> bool {
        self.dev.send(eth).is_ok()
    }

    fn recv(&mut self) -> Option<Vec<u8>> {
        self.dev.recv()
    }

    fn fd(&self) -> Option<usize> {
        self.dev.bus.sdio.irq
    }

    fn pending(&self) -> bool {
        self.dev.pending()
    }
}

/// What to tell the user when the radio `chip` did not come up.
pub fn why(chip: &Known, f: &Failed) -> Vec<u8> {
    let mut s = format!("wifi: {}: ", chip.name).into_bytes();
    match f {
        Failed::Sdio(what) => s.extend_from_slice(what.as_bytes()),
        Failed::NoBoard(model) => {
            s.extend_from_slice(b"no NVRAM for the board ");
            s.extend_from_slice(if model.is_empty() { b"(unknown)" } else { model });
        }
        Failed::NoFirmware(name) => s.extend_from_slice(format!("no {name} (wifi-firmware fetch)").as_bytes()),
        Failed::Driver(e) => s.extend_from_slice(format!("{e:?}").as_bytes()),
    }
    s.push(b'\n');
    s
}
