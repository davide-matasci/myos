//! A USB device driven through `/dev/usb/NNN` (modules/usbdev,
//! docs/usb.md): vendor requests through `ep0`, writes to its bulk OUT
//! endpoints, reads of its bulk IN one, for the MediaTek radios' driver
//! (the `mtw` crate).

use alloc::vec::Vec;

use mtw::usb::{Usb as Bus, UsbError};
use myos_user::{close, monotonic_ns, open_flags, read, sleep_ns, write_fd, O_RDWR};

use crate::devs::{self, hex, path, slurp};

pub struct Usb {
    dir: Vec<u8>,
    ctl: usize,
    ep0: usize,
    /// The bulk endpoints in the interface's order, and the fd of each
    /// OUT one (opened when first written).
    pub bulk: Vec<u8>,
    outs: Vec<(u8, usize)>,
}

/// The first device under `/dev/usb` whose `id VVVV:PPPP` is one of
/// `ids`: its directory and id.
pub fn find(ids: &[(u16, u16)]) -> Option<(Vec<u8>, (u16, u16))> {
    devs::find(b"/dev/usb/", ids)
}

impl Usb {
    /// Hold the device at `dir`: its endpoint files appear.
    pub fn open(dir: &[u8]) -> Result<Self, &'static str> {
        let ctl = open_flags(&path(dir, b"/ctl", b""), O_RDWR).ok_or("its ctl does not open (driven, or not root)")?;
        let Some(ep0) = open_flags(&path(dir, b"/ep0", b""), O_RDWR) else {
            close(ctl);
            return Err("ep0");
        };
        let mut u = Usb { dir: dir.to_vec(), ctl, ep0, bulk: Vec::new(), outs: Vec::new() };
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
        for &(_, fd) in &self.outs {
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

    fn bulk_out(&mut self, endpoint: u8, data: &[u8]) -> Result<(), UsbError> {
        let fd = match self.outs.iter().find(|(a, _)| *a == endpoint) {
            Some(&(_, fd)) => fd,
            None => {
                let fd = self.endpoint(endpoint, O_RDWR).ok_or(UsbError)?;
                self.outs.push((endpoint, fd));
                fd
            }
        };
        (write_fd(fd, data) == data.len()).then_some(()).ok_or(UsbError)
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
