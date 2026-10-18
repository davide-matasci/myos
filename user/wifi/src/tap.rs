//! The link's NIC for netd: a virtual NIC of `/dev/tap` (`modules/tap`),
//! held while the station is associated. netd takes it once its link is
//! up and leases an address on it (`user/netd/src/nics.rs`); what netd
//! sends on it goes out as 802.11 data frames, and those the access point
//! sends the station come back in on it.

use alloc::vec::Vec;

use myos_user::{close, open_flags, poll, read, write_fd, PollFd, O_RDWR, POLLIN};

/// The virtual NICs there are (`/dev/tap/0` to `3`).
const TAPS: u8 = 4;

pub struct Tap {
    prog: usize,
    ctl: usize,
}

impl Tap {
    /// Make a free NIC with address `mac`, its link up.
    pub fn open(mac: [u8; 6]) -> Option<Self> {
        let (prog, n) = (0..TAPS).find_map(|n| open_flags(&path(n, b"prog"), O_RDWR).map(|fd| (fd, n)))?;
        let Some(ctl) = open_flags(&path(n, b"ctl"), O_RDWR) else {
            close(prog);
            return None;
        };
        let mut line = Vec::from(b"mac " as &[u8]);
        for (i, b) in mac.iter().enumerate() {
            if i != 0 {
                line.push(b':');
            }
            line.push(b"0123456789abcdef"[(b >> 4) as usize]);
            line.push(b"0123456789abcdef"[(b & 15) as usize]);
        }
        let tap = Tap { prog, ctl };
        if write_fd(ctl, &line) != line.len() || write_fd(ctl, b"up") != 2 {
            tap.close();
            return None;
        }
        Some(tap)
    }

    /// A frame netd sent, if one is waiting (a read of `prog` waits for
    /// one: `poll` first).
    pub fn recv(&self, buf: &mut [u8]) -> Option<usize> {
        let mut fds = [PollFd { fd: self.prog as i32, events: POLLIN, revents: 0 }];
        if poll(&mut fds, 0) != 1 {
            return None;
        }
        let n = read(self.prog, buf);
        (n != 0 && n != usize::MAX).then_some(n)
    }

    /// Hand netd an Ethernet frame.
    pub fn send(&self, frame: &[u8]) {
        write_fd(self.prog, frame);
    }

    /// The fd a `poll` for [`Tap::recv`] waits on.
    pub fn fd(&self) -> usize {
        self.prog
    }

    /// Link down and the NIC gone: netd lets go of it.
    pub fn close(self) {
        write_fd(self.ctl, b"down");
        close(self.ctl);
        close(self.prog);
    }
}

fn path(n: u8, file: &[u8]) -> Vec<u8> {
    let mut p = Vec::from(b"/dev/tap/" as &[u8]);
    p.push(b'0' + n);
    p.push(b'/');
    p.extend_from_slice(file);
    p
}
