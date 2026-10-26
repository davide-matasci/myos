//! The server's socket: `/net/unix` (`docs/sockets-unix.md`), a listener
//! announced by `main` and the connections of the programs to it.

use myos_user::{close, open_flags, poll, read, write_fd, PollFd, O_RDWR, POLLIN};


/// A conversation of `/net/unix`: its `ctl` and `data`, and its number.
pub struct Conv {
    pub n: u32,
    pub ctl: usize,
    pub data: usize,
}

fn path<'a>(out: &'a mut [u8; 48], n: u32, file: &[u8]) -> &'a [u8] {
    let mut len = 0;
    let mut push = |s: &[u8], out: &mut [u8; 48]| {
        out[len..len + s.len()].copy_from_slice(s);
        len += s.len();
    };
    push(b"/net/unix/", out);
    let mut d = [0u8; 10];
    let mut i = d.len();
    let mut v = n;
    loop {
        i -= 1;
        d[i] = b'0' + (v % 10) as u8;
        v /= 10;
        if v == 0 {
            break;
        }
    }
    push(&d[i..], out);
    push(b"/", out);
    push(file, out);
    &out[..len]
}

fn number(text: &[u8]) -> Option<u32> {
    let t = text.trim_ascii();
    if t.is_empty() || !t.iter().all(u8::is_ascii_digit) {
        return None;
    }
    core::str::from_utf8(t).ok()?.parse().ok()
}

impl Conv {
    /// Conversation `n`'s files.
    pub fn open(n: u32) -> Option<Conv> {
        let mut p = [0u8; 48];
        let ctl = open_flags(path(&mut p, n, b"ctl"), O_RDWR)?;
        let Some(data) = open_flags(path(&mut p, n, b"data"), O_RDWR) else {
            close(ctl);
            return None;
        };
        Some(Conv { n, ctl, data })
    }

    /// A new conversation, `cmd` written to its `ctl` (`announce NAME`,
    /// `connect NAME`). Opening `clone` makes it: that fd is its `data`.
    pub fn new(cmd: &[u8]) -> Option<Conv> {
        let (data, n) = myos_user::clone_conv(b"/net/unix")?;
        let mut p = [0u8; 48];
        let Some(ctl) = open_flags(path(&mut p, u32::from(n), b"ctl"), O_RDWR) else {
            close(data);
            return None;
        };
        let conv = Conv { n: u32::from(n), ctl, data };
        if write_fd(conv.ctl, cmd) != cmd.len() {
            conv.close();
            return None;
        }
        Some(conv)
    }

    /// A listener's next queued connection, if one waits.
    pub fn accept(&self) -> Option<Conv> {
        let mut p = [0u8; 48];
        let fd = open_flags(path(&mut p, self.n, b"listen"), O_RDWR)?;
        let mut buf = [0u8; 16];
        let k = read(fd, &mut buf);
        close(fd);
        buf.get(..k).and_then(number).and_then(Conv::open)
    }

    /// Read into `buf`, waiting up to `timeout_ms` for the first byte; 0 at
    /// the end (the peer gone) or when nothing came.
    pub fn read(&self, buf: &mut [u8], timeout_ms: i32) -> usize {
        let mut fds = [PollFd { fd: self.data as i32, events: POLLIN, revents: 0 }];
        if poll(&mut fds, timeout_ms) != 1 {
            return 0;
        }
        let n = read(self.data, buf);
        if n == usize::MAX { 0 } else { n }
    }

    /// Write all of `buf` (the peer's buffer holds 64 KiB: a reply fits).
    pub fn write(&self, mut buf: &[u8]) -> bool {
        while !buf.is_empty() {
            let n = write_fd(self.data, buf);
            if n == 0 || n == usize::MAX {
                return false;
            }
            buf = &buf[n..];
        }
        true
    }

    pub fn close(self) {
        close(self.data);
        close(self.ctl);
    }
}
