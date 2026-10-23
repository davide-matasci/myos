//! MediaTek's USB radios: the MT7601U, as `mtw::chip` describes it, with
//! the `mtw` crate's driver over `/dev/usb` and its firmware from
//! `wifi-firmware`. A SoftMAC: the daemon scans by visiting the channels
//! (Radio::scan is the default), joins and encrypts; the radio tunes,
//! sends and hears.

use alloc::collections::VecDeque;
use alloc::vec::Vec;

use mtw::chip::{CHIPS, Chip};
use mtw::dev::{Dev, Error, Info};
use mtw::fw::Firmware;
use mtw::reg::MAX_RXSZ;
use mtw::rx;
use mtw::usb::Endpoints;
use myos_user::{poll, read, PollFd, O_RDONLY, POLLIN};

use crate::devs::load;
use crate::frame::Bss;
use crate::radio::{Radio, Rx};
use crate::usb::{self, Usb};

/// Where `wifi-firmware fetch` keeps the firmware by default.
const FIRMWARE_DIRS: &[&[u8]] = &[b"/data/firmware/", b"/tmp/firmware/"];

pub struct Mediatek {
    dev: Dev<Usb>,
    info: Info,
    /// The bulk IN endpoint's file: a read is one transfer of frames.
    rx: usize,
    channel: u8,
    /// The frames of the last transfer not handed out yet: {frame, dBm}.
    heard: VecDeque<(Vec<u8>, i8)>,
    /// The network being joined, or joined.
    bssid: Option<[u8; 6]>,
}

/// Why the radio did not come up, for the daemon to say.
pub enum Failed {
    Usb(&'static str),
    NoFirmware,
    BadFirmware,
    Driver(Error),
}

impl Mediatek {
    /// The radio `chip` at `/dev/usb` directory `dir`: held, its firmware
    /// loaded and started, up on channel 1.
    pub fn open(dir: &[u8], chip: &'static Chip) -> Result<Self, Failed> {
        let name = chip.firmware.as_bytes();
        let file = FIRMWARE_DIRS.iter().find_map(|d| load(&[d, name].concat())).ok_or(Failed::NoFirmware)?;
        let fw = Firmware::parse(&file).map_err(|_| Failed::BadFirmware)?;
        let usb = Usb::open(dir).map_err(Failed::Usb)?;
        let eps = Endpoints::new(&usb.bulk).ok_or(Failed::Usb("not its endpoints"))?;
        let rx = usb.endpoint(eps.rx, O_RDONLY).ok_or(Failed::Usb("its bulk IN endpoint"))?;
        let mut dev = Dev::new(usb, chip, eps).map_err(Failed::Driver)?;
        let info = dev.attach(&fw).map_err(Failed::Driver)?;
        dev.init(info.mac).map_err(Failed::Driver)?;
        Ok(Mediatek { dev, info, rx, channel: 1, heard: VecDeque::new(), bssid: None })
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
        let dev = &self.dev;
        let heard = &mut self.heard;
        rx::frames(&mut xfer[..n], |f| heard.push_back((f.frame.to_vec(), dev.rssi_dbm(f.rssi))));
        true
    }
}

/// The first of the radios in `/dev/usb`, if there is one: its directory
/// and which it is.
pub fn find() -> Option<(Vec<u8>, &'static Chip)> {
    let ids: Vec<(u16, u16)> = CHIPS.iter().flat_map(|c| c.ids.iter().copied()).collect();
    let (dir, id) = usb::find(&ids)?;
    Some((dir, Chip::by_id(id)?))
}

impl Radio for Mediatek {
    fn mac(&self) -> [u8; 6] {
        self.info.mac
    }

    /// What was heard on the channel before is dropped.
    fn set_channel(&mut self, channel: u8) {
        if self.dev.set_chan(channel).is_ok() {
            self.channel = channel;
            while self.fill() {}
            self.heard.clear();
        }
    }

    fn send(&mut self, frame: &[u8]) -> bool {
        self.dev.send(frame).is_ok()
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

    /// Tuned to `bss`'s channel, its address the MAC's BSSID.
    fn joining(&mut self, bss: &Bss) -> bool {
        self.left();
        if self.dev.joining(bss.bssid, bss.channel).is_err() {
            return false;
        }
        self.channel = bss.channel;
        self.bssid = Some(bss.bssid);
        true
    }

    fn associated(&mut self, aid: u16) -> bool {
        self.bssid.is_some_and(|b| self.dev.associated(b, aid).is_ok())
    }

    fn left(&mut self) {
        if self.bssid.take().is_some() {
            let _ = self.dev.left();
        }
    }
}

/// What to tell the user when the radio `chip` did not come up.
pub fn why(chip: &Chip, f: &Failed) -> Vec<u8> {
    let mut s = alloc::format!("wifi: {}: ", chip.name).into_bytes();
    match f {
        Failed::Usb(what) => s.extend_from_slice(what.as_bytes()),
        Failed::NoFirmware => s.extend_from_slice(alloc::format!("no {} (wifi-firmware fetch)", chip.firmware).as_bytes()),
        Failed::BadFirmware => s.extend_from_slice(b"its firmware file is not one (wifi-firmware fetch)"),
        Failed::Driver(e) => s.extend_from_slice(alloc::format!("{e:?}").as_bytes()),
    }
    s.push(b'\n');
    s
}
