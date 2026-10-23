//! An SDIO card driven through `/dev/sdio/N` (modules/sdmmc, "SDIO" in
//! docs/raspberry-pi.md): its functions' registers through `fN` (the file
//! position the register address) and `fNfifo`, the block sizes and the
//! card's interrupt through `ctl` and `irq`, for the Broadcom radios'
//! driver (the `bwfm` crate).

use alloc::format;
use alloc::vec::Vec;

use bwfm::sdio::{Sdio as Bus, SdioError};
use myos_user::{close, lseek, monotonic_ns, open_flags, poll, read, sleep_ns, write_fd, PollFd, O_RDWR, POLLIN, SEEK_SET};

use crate::devs::path;

/// A byte-mode transfer's longest.
const BYTE_MODE_MAX: usize = 512;

pub struct Sdio {
    dir: Vec<u8>,
    ctl: usize,
    /// `fN` and `fNfifo` of functions 0 to 2, opened at their first use.
    files: [Option<usize>; 6],
    /// The functions' block sizes, as set.
    block: [u16; 3],
    /// `irq`, once the card's interrupt is wanted (a `poll` of it waits
    /// for the interrupt).
    pub irq: Option<usize>,
}

impl Sdio {
    /// Hold the card at `dir`: its function files appear.
    pub fn open(dir: &[u8]) -> Result<Self, &'static str> {
        let ctl = open_flags(&path(dir, b"/ctl", b""), O_RDWR).ok_or("its ctl does not open (driven, or not root)")?;
        Ok(Sdio { dir: dir.to_vec(), ctl, files: [None; 6], block: [0; 3], irq: None })
    }

    /// Function `func`'s register file, at a fixed address when `fifo`,
    /// positioned at `addr`.
    fn at(&mut self, func: u8, fifo: bool, addr: u32) -> Result<usize, SdioError> {
        let i = 2 * usize::from(func) + usize::from(fifo);
        if i >= self.files.len() {
            return Err(SdioError);
        }
        let fd = match self.files[i] {
            Some(fd) => fd,
            None => {
                let name = if fifo { format!("/f{func}fifo") } else { format!("/f{func}") };
                let fd = open_flags(&path(&self.dir, name.as_bytes(), b""), O_RDWR).ok_or(SdioError)?;
                self.files[i] = Some(fd);
                fd
            }
        };
        (lseek(fd, addr as usize, SEEK_SET) == addr as usize).then_some(fd).ok_or(SdioError)
    }

    /// How much of the `left` bytes of a transfer on `func` the next CMD53
    /// moves: all of it in byte mode, or whole blocks (two at least: one
    /// is byte mode's) when it is longer than byte mode allows. The card
    /// takes a FIFO's data in pieces, as every host splits it.
    fn piece(&self, func: u8, left: usize) -> Result<usize, SdioError> {
        if left <= BYTE_MODE_MAX {
            return Ok(left);
        }
        let block = usize::from(*self.block.get(usize::from(func)).ok_or(SdioError)?);
        if block == 0 {
            return Err(SdioError);
        }
        Ok(match left / block {
            0 | 1 => left.min(BYTE_MODE_MAX),
            count => count * block,
        })
    }

    fn control(&mut self, cmd: &[u8]) -> bool {
        write_fd(self.ctl, cmd) == cmd.len()
    }

    /// The card's interrupt from now on, if the slot has a line: `irq`.
    pub fn irq_on(&mut self) -> Option<usize> {
        let fd = open_flags(&path(&self.dir, b"/irq", b""), O_RDWR)?;
        if !self.control(b"irq on") {
            close(fd);
            return None;
        }
        self.irq = Some(fd);
        Some(fd)
    }

    /// The interrupt, if it came: how many since the last time (a read of
    /// `irq` would wait for one, so only when `poll` shows it pending).
    pub fn irq_take(&mut self) -> u64 {
        let Some(fd) = self.irq else {
            return 0;
        };
        let mut fds = [PollFd { fd: fd as i32, events: POLLIN, revents: 0 }];
        if poll(&mut fds, 0) != 1 {
            return 0;
        }
        let mut n = [0u8; 8];
        if read(fd, &mut n) != 8 {
            return 0;
        }
        u64::from_le_bytes(n)
    }

    /// The card serviced: its interrupt may fire again.
    pub fn irq_rearm(&mut self) {
        if let Some(fd) = self.irq {
            write_fd(fd, &[0]);
        }
    }
}

impl Drop for Sdio {
    fn drop(&mut self) {
        for fd in self.files.iter().flatten() {
            close(*fd);
        }
        if let Some(fd) = self.irq {
            close(fd);
        }
        close(self.ctl);
    }
}

impl Bus for Sdio {
    fn read_1(&mut self, func: u8, addr: u32) -> Result<u8, SdioError> {
        let fd = self.at(func, false, addr)?;
        let mut b = [0u8];
        (read(fd, &mut b) == 1).then_some(b[0]).ok_or(SdioError)
    }

    fn write_1(&mut self, func: u8, addr: u32, value: u8) -> Result<(), SdioError> {
        let fd = self.at(func, false, addr)?;
        (write_fd(fd, &[value]) == 1).then_some(()).ok_or(SdioError)
    }

    fn read(&mut self, func: u8, addr: u32, buf: &mut [u8], incr: bool) -> Result<(), SdioError> {
        let mut done = 0;
        while done < buf.len() {
            let n = self.piece(func, buf.len() - done)?;
            let fd = self.at(func, !incr, if incr { addr + done as u32 } else { addr })?;
            if read(fd, &mut buf[done..done + n]) != n {
                return Err(SdioError);
            }
            done += n;
        }
        Ok(())
    }

    fn write(&mut self, func: u8, addr: u32, data: &[u8], incr: bool) -> Result<(), SdioError> {
        let mut done = 0;
        while done < data.len() {
            let n = self.piece(func, data.len() - done)?;
            let fd = self.at(func, !incr, if incr { addr + done as u32 } else { addr })?;
            if write_fd(fd, &data[done..done + n]) != n {
                return Err(SdioError);
            }
            done += n;
        }
        Ok(())
    }

    fn set_block_size(&mut self, func: u8, size: u16) -> Result<(), SdioError> {
        let slot = self.block.get_mut(usize::from(func)).ok_or(SdioError)?;
        *slot = size;
        let cmd = format!("blocksize {func} {size}");
        self.control(cmd.as_bytes()).then_some(()).ok_or(SdioError)
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

    fn now_ms(&mut self) -> u64 {
        monotonic_ns() / 1_000_000
    }
}
