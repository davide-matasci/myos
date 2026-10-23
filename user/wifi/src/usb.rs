//! A USB device driven through `/dev/usb/NNN` (modules/usbdev,
//! docs/usb.md): vendor and class requests through `ep0`, writes to its
//! bulk OUT endpoints, reads of its bulk IN ones, for the MediaTek radios'
//! driver (the `mtw` crate).

use alloc::vec::Vec;

use mtw::usb::{Usb as Bus, UsbError};
use myos_user::{
    close, listdir, monotonic_ns, open_flags, poll, read, sleep_ns, write_fd, PollFd, LISTDIR_BUF, O_RDONLY, O_RDWR, POLLIN,
};

pub struct Usb {
    dir: Vec<u8>,
    ctl: usize,
    ep0: usize,
    /// The bulk endpoints in the interface's order, and the fd of each
    /// one the driver uses (opened when first written or read).
    pub bulk: Vec<u8>,
    files: Vec<(u8, usize)>,
}

/// The first device under `/dev/usb` whose `id VVVV:PPPP` is one of
/// `ids`: its directory and id.
pub fn find(ids: &[(u16, u16)]) -> Option<(Vec<u8>, (u16, u16))> {
    let mut names = [0u8; LISTDIR_BUF];
    let n = listdir(b"/dev/usb", &mut names);
    if n == usize::MAX {
        return None;
    }
    for name in names[..n].split(|&c| c == b'\n' || c == 0).filter(|s| !s.is_empty() && s[0] != b'.') {
        let dir = path(b"/dev/usb/", name, b"");
        let Some(text) = slurp(&path(&dir, b"/info", b"")) else {
            continue;
        };
        let Some(id) = value(&text, b"id ") else {
            continue;
        };
        let mut parts = id.split(|&c| c == b':');
        let (Some(v), Some(p)) = (parts.next().and_then(hex), parts.next().and_then(hex)) else {
            continue;
        };
        if let Some(&id) = ids.iter().find(|&&id| id == (v as u16, p as u16)) {
            return Some((dir, id));
        }
    }
    None
}

impl Usb {
    /// Hold the device at `dir`: its endpoint files appear.
    pub fn open(dir: &[u8]) -> Result<Self, &'static str> {
        let ctl = open_flags(&path(dir, b"/ctl", b""), O_RDWR).ok_or("its ctl does not open (driven, or not root)")?;
        let Some(ep0) = open_flags(&path(dir, b"/ep0", b""), O_RDWR) else {
            close(ctl);
            return Err("ep0");
        };
        let mut u = Usb { dir: dir.to_vec(), ctl, ep0, bulk: Vec::new(), files: Vec::new() };
        let text = slurp(&path(dir, b"/info", b"")).ok_or("info")?;
        // `epXX bulk in|out MAXPACKET` lines.
        for l in text.split(|&c| c == b'\n') {
            let mut w = l.split(|&c| c == b' ');
            if let (Some(ep), Some(b"bulk")) = (w.next().and_then(|e| e.strip_prefix(b"ep")), w.next())
                && let Some(a) = hex(ep)
            {
                u.bulk.push(a as u8);
            }
        }
        Ok(u)
    }

    /// Endpoint `address`'s file, opened `flags`.
    pub fn endpoint(&self, address: u8, flags: u32) -> Option<usize> {
        let name = [b'/', b'e', b'p', hex_digit(address >> 4), hex_digit(address & 15)];
        open_flags(&path(&self.dir, &name, b""), flags)
    }

    /// Endpoint `address`'s file, opened the first time.
    fn held(&mut self, address: u8) -> Result<usize, UsbError> {
        if let Some(&(_, fd)) = self.files.iter().find(|(a, _)| *a == address) {
            return Ok(fd);
        }
        let fd = self.endpoint(address, if address & 0x80 != 0 { O_RDONLY } else { O_RDWR }).ok_or(UsbError)?;
        self.files.push((address, fd));
        Ok(fd)
    }

    /// One control transfer: setup packet, then an OUT data stage.
    fn control(&mut self, request_type: u8, request: u8, value: u16, index: u16, len: usize, out: &[u8]) -> bool {
        let mut x = Vec::with_capacity(8 + out.len());
        x.extend_from_slice(&[request_type, request]);
        x.extend_from_slice(&value.to_le_bytes());
        x.extend_from_slice(&index.to_le_bytes());
        x.extend_from_slice(&(len as u16).to_le_bytes());
        x.extend_from_slice(out);
        write_fd(self.ep0, &x) == x.len()
    }
}

impl Drop for Usb {
    fn drop(&mut self) {
        for &(_, fd) in &self.files {
            close(fd);
        }
        close(self.ep0);
        close(self.ctl);
    }
}

impl Bus for Usb {
    fn vendor_out(&mut self, request: u8, value: u16, index: u16, data: &[u8]) -> Result<(), UsbError> {
        self.control(0x40, request, value, index, data.len(), data).then_some(()).ok_or(UsbError)
    }

    /// The data stage is the next read of `ep0`.
    fn vendor_in(&mut self, request: u8, value: u16, index: u16, data: &mut [u8]) -> Result<(), UsbError> {
        if !self.control(0xc0, request, value, index, data.len(), &[]) {
            return Err(UsbError);
        }
        (read(self.ep0, data) == data.len()).then_some(()).ok_or(UsbError)
    }

    fn class_out(&mut self, request: u8, value: u16, index: u16, data: &[u8]) -> Result<(), UsbError> {
        self.control(0x20, request, value, index, data.len(), data).then_some(()).ok_or(UsbError)
    }

    fn bulk_out(&mut self, endpoint: u8, data: &[u8]) -> Result<(), UsbError> {
        let fd = self.held(endpoint)?;
        (write_fd(fd, data) == data.len()).then_some(()).ok_or(UsbError)
    }

    /// The endpoint's file keeps a transfer queued once opened: a poll
    /// waits for it.
    fn bulk_in(&mut self, endpoint: u8, buf: &mut [u8], timeout_ms: u32) -> Result<usize, UsbError> {
        let fd = self.held(endpoint)?;
        let mut fds = [PollFd { fd: fd as i32, events: POLLIN, revents: 0 }];
        if poll(&mut fds, timeout_ms as i32) != 1 {
            return Ok(0);
        }
        match read(fd, buf) {
            usize::MAX | 0 => Err(UsbError),
            n => Ok(n),
        }
    }

    fn delay_us(&mut self, us: u32) {
        if us >= 1000 {
            sleep_ns(u64::from(us) * 1000, false);
            return;
        }
        let end = monotonic_ns() + u64::from(us) * 1000;
        while monotonic_ns() < end {
            core::hint::spin_loop();
        }
    }
}

fn hex_digit(v: u8) -> u8 {
    if v < 10 { b'0' + v } else { b'a' + v - 10 }
}

fn path(a: &[u8], b: &[u8], c: &[u8]) -> Vec<u8> {
    let mut p = Vec::with_capacity(a.len() + b.len() + c.len());
    p.extend_from_slice(a);
    p.extend_from_slice(b);
    p.extend_from_slice(c);
    p
}

fn slurp(path: &[u8]) -> Option<Vec<u8>> {
    let fd = open_flags(path, O_RDONLY)?;
    let mut buf = [0u8; 1024];
    let n = read(fd, &mut buf);
    close(fd);
    (n != usize::MAX).then(|| buf[..n].to_vec())
}

/// What follows `key` on its line of `text`.
fn value<'a>(text: &'a [u8], key: &[u8]) -> Option<&'a [u8]> {
    text.split(|&c| c == b'\n').find_map(|l| l.strip_prefix(key)).map(|v| v.trim_ascii())
}

fn hex(s: &[u8]) -> Option<u64> {
    u64::from_str_radix(core::str::from_utf8(s).ok()?, 16).ok()
}
