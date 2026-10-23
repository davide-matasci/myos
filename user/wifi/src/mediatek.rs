//! MediaTek's USB radios, as `mtw::chip` describes them: the MT7601U with
//! the `mtw` crate's `dev`, the MT7612U with its `mt76x2`, over
//! `/dev/usb`, their firmware from `wifi-firmware`. SoftMACs: the daemon
//! scans by visiting the channels (Radio::scan is the default; the
//! MT7612U's include 5 GHz), joins and encrypts; the radio tunes, sends
//! and hears.

use alloc::collections::VecDeque;
use alloc::vec::Vec;

use mtw::chip::{CHIPS, Chip, Family};
use mtw::dev::{Dev, Error, Info, Peer};
use mtw::mt76x2;
use mtw::reg::MAX_RXSZ;
use mtw::rx;
use mtw::usb::Endpoints;
use myos_user::{close, open_flags, poll, read, PollFd, O_RDONLY, POLLIN};

use crate::frame::Bss;
use crate::radio::{Radio, Rx};
use crate::usb::{self, Usb};

/// Where `wifi-firmware fetch` keeps the firmware by default.
const FIRMWARE_DIRS: &[&[u8]] = &[b"/data/firmware/", b"/tmp/firmware/"];

/// The chip's driver.
enum Driver {
    Mt7601(Dev<Usb>),
    Mt76x2(mt76x2::dev::Dev<Usb>),
}

pub struct Mediatek {
    dev: Driver,
    info: Info,
    /// The bulk IN endpoint's file: a read is one transfer of frames.
    rx: usize,
    channel: u8,
    /// The frames of the last transfer not handed out yet: {frame, dBm}.
    heard: VecDeque<(Vec<u8>, i8)>,
    /// The network being joined, or joined.
    peer: Option<Peer>,
}

/// Why the radio did not come up, for the daemon to say.
pub enum Failed {
    Usb(&'static str),
    /// The file named is not in the firmware directory, or not one.
    NoFirmware(&'static str),
    BadFirmware(&'static str),
    Driver(Error),
}

impl Mediatek {
    /// The radio `chip` at `/dev/usb` directory `dir`: held, its firmware
    /// loaded and started, up on its first channel.
    pub fn open(dir: &[u8], chip: &'static Chip) -> Result<Self, Failed> {
        let file = firmware(chip.firmware)?;
        let patch = match chip.rom_patch {
            Some(name) => Some((name, firmware(name)?)),
            None => None,
        };
        let usb = Usb::open(dir).map_err(Failed::Usb)?;
        let eps = Endpoints::new(&usb.bulk).ok_or(Failed::Usb("not its endpoints"))?;
        let rx = usb.endpoint(eps.rx, O_RDONLY).ok_or(Failed::Usb("its bulk IN endpoint"))?;
        let bad = |name| move |_| Failed::BadFirmware(name);
        let (dev, info) = match (chip.family, patch) {
            (Family::Mt7601, _) => {
                let fw = mtw::fw::Firmware::parse(&file).map_err(bad(chip.firmware))?;
                let mut dev = Dev::new(usb, chip, eps).map_err(Failed::Driver)?;
                let info = dev.attach(&fw).map_err(Failed::Driver)?;
                dev.init(info.mac).map_err(Failed::Driver)?;
                (Driver::Mt7601(dev), info)
            }
            (Family::Mt76x2, Some((name, patch))) => {
                let fw = mt76x2::fw::Firmware::parse(&file).map_err(bad(chip.firmware))?;
                let patch = mt76x2::fw::RomPatch::parse(&patch).map_err(bad(name))?;
                let mut dev = mt76x2::dev::Dev::new(usb, chip, eps).map_err(Failed::Driver)?;
                let info = dev.attach(&patch, &fw).map_err(Failed::Driver)?;
                dev.init(info.mac).map_err(Failed::Driver)?;
                (Driver::Mt76x2(dev), info)
            }
            (Family::Mt76x2, None) => return Err(Failed::NoFirmware("its ROM patch")),
        };
        let channel = match &dev {
            Driver::Mt7601(_) => 1,
            Driver::Mt76x2(d) => d.channels()[0],
        };
        Ok(Mediatek { dev, info, rx, channel, heard: VecDeque::new(), peer: None })
    }

    /// The transfer waiting on the bulk IN endpoint, if one is: its frames
    /// kept.
    fn fill(&mut self) -> bool {
        let mut fds = [PollFd { fd: self.rx as i32, events: POLLIN, revents: 0 }];
        if poll(&mut fds, 0) != 1 {
            return false;
        }
        let mut xfer = alloc::vec![0u8; MAX_RXSZ];
        let n = read(self.rx, &mut xfer);
        if n == 0 || n == usize::MAX {
            return false;
        }
        let heard = &mut self.heard;
        match &mut self.dev {
            Driver::Mt7601(d) => rx::frames(&mut xfer[..n], |f| heard.push_back((f.frame.to_vec(), d.rssi_dbm(f.rssi)))),
            Driver::Mt76x2(d) => d.frames(&mut xfer[..n], |f, dbm| heard.push_back((f.to_vec(), dbm))),
        }
        true
    }

    fn chip(&self) -> &'static Chip {
        match &self.dev {
            Driver::Mt7601(d) => d.chip,
            Driver::Mt76x2(d) => d.chip,
        }
    }
}

/// The first of the radios in `/dev/usb`, if there is one: its directory
/// and which it is.
pub fn find() -> Option<(Vec<u8>, &'static Chip)> {
    let ids: Vec<(u16, u16)> = CHIPS.iter().flat_map(|c| c.ids.iter().copied()).collect();
    let (dir, id) = usb::find(&ids)?;
    Some((dir, Chip::by_id(id)?))
}

/// Firmware file `name` from the first firmware directory that has it.
fn firmware(name: &'static str) -> Result<Vec<u8>, Failed> {
    FIRMWARE_DIRS.iter().find_map(|d| load(&[d, name.as_bytes()].concat())).ok_or(Failed::NoFirmware(name))
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

impl Radio for Mediatek {
    fn mac(&self) -> [u8; 6] {
        self.info.mac
    }

    /// The MT7612U's: its board's bands.
    fn channels(&self) -> Vec<u8> {
        match &self.dev {
            Driver::Mt7601(_) => (1..=13).collect(),
            Driver::Mt76x2(d) => d.channels(),
        }
    }

    /// What was heard on the channel before is dropped.
    fn set_channel(&mut self, channel: u8) {
        let tuned = match &mut self.dev {
            Driver::Mt7601(d) => d.set_chan(channel),
            Driver::Mt76x2(d) => d.set_chan(channel),
        };
        if tuned.is_ok() {
            self.channel = channel;
            while self.fill() {}
            self.heard.clear();
        }
    }

    fn send(&mut self, frame: &[u8]) -> bool {
        match &mut self.dev {
            Driver::Mt7601(d) => d.send(frame).is_ok(),
            Driver::Mt76x2(d) => d.send(frame).is_ok(),
        }
    }

    fn recv(&mut self, buf: &mut [u8]) -> Option<Rx> {
        if self.heard.is_empty() {
            self.fill();
        }
        let (frame, rssi) = self.heard.pop_front()?;
        let len = frame.len().min(buf.len());
        buf[..len].copy_from_slice(&frame[..len]);
        Some(Rx { len, channel: self.channel, rssi })
    }

    fn fd(&self) -> usize {
        self.rx
    }

    fn pending(&self) -> bool {
        !self.heard.is_empty()
    }

    /// Tuned to `bss`'s channel (calibrated for it), its address the MAC's
    /// BSSID.
    fn joining(&mut self, bss: &Bss) -> bool {
        self.left();
        let peer = Peer {
            bssid: bss.bssid,
            channel: bss.channel,
            rates: bss.rates.clone(),
            ht_mcs: bss.ht_mcs,
            short_preamble: bss.short_preamble,
            short_slot: bss.short_slot,
        };
        let r = match &mut self.dev {
            Driver::Mt7601(d) => d.joining(&peer),
            Driver::Mt76x2(d) => d.joining(&peer),
        };
        if r.is_err() {
            return false;
        }
        self.channel = bss.channel;
        self.peer = Some(peer);
        true
    }

    /// The access point's rates for AMRR: its MCS over 802.11n.
    fn associated(&mut self, aid: u16, ht: bool) -> bool {
        let Some(p) = self.peer.as_ref() else {
            return false;
        };
        match &mut self.dev {
            Driver::Mt7601(d) => d.associated(p, aid, ht).is_ok(),
            Driver::Mt76x2(d) => d.associated(p, aid, ht).is_ok(),
        }
    }

    fn left(&mut self) {
        if self.peer.take().is_some() {
            let _ = match &mut self.dev {
                Driver::Mt7601(d) => d.left(),
                Driver::Mt76x2(d) => d.left(),
            };
        }
    }

    fn ht(&self) -> Option<[u8; 26]> {
        Some(self.chip().ht_cap)
    }

    /// AMRR's once-a-second look at the TX statistics (and the MT7612U's
    /// periodic calibration).
    fn tick(&mut self) {
        let _ = match &mut self.dev {
            Driver::Mt7601(d) => d.calibrate(),
            Driver::Mt76x2(d) => d.calibrate(),
        };
    }
}

/// What to tell the user when the radio `chip` did not come up.
pub fn why(chip: &Chip, f: &Failed) -> Vec<u8> {
    let mut s = alloc::format!("wifi: {}: ", chip.name).into_bytes();
    match f {
        Failed::Usb(what) => s.extend_from_slice(what.as_bytes()),
        Failed::NoFirmware(name) => s.extend_from_slice(alloc::format!("no {name} (wifi-firmware fetch)").as_bytes()),
        Failed::BadFirmware(name) => s.extend_from_slice(alloc::format!("{name} is not its firmware (wifi-firmware fetch)").as_bytes()),
        Failed::Driver(e) => s.extend_from_slice(alloc::format!("{e:?}").as_bytes()),
    }
    s.push(b'\n');
    s
}
