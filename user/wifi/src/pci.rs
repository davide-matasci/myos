//! A PCI function driven through `/dev/pci/BB:SS.F` (modules/pcidev,
//! docs/pci.md): its registers (BAR0), DMA buffers and interrupt
//! for the Intel radios' driver (the `iwx` crate).

use alloc::vec::Vec;

use iwx::dev::{Device, Dma};
use iwx::hw::Bus;
use myos_user::{
    close, listdir, mmap, monotonic_ns, open_flags, poll, read, read_at, sleep_ns, write_at, write_fd,
    PollFd, LISTDIR_BUF, MAP_SHARED, O_RDONLY, O_RDWR, POLLIN, PROT_READ, PROT_WRITE,
};

pub struct Pci {
    dir: Vec<u8>,
    ctl: usize,
    regs: *mut u8,
    regs_len: usize,
    irq: usize,
    /// The interrupt is MSI-X (else MSI, or the INTx line).
    pub msix: bool,
    /// The INTx line: masked by /dev/pci from its interrupt until the
    /// driver quietened the device (a write to `irq`).
    intx: bool,
}

/// The first function under `/dev/pci` whose `id VVVV:DDDD` is one of
/// `ids`: its directory and id.
pub fn find(ids: &[(u16, u16)]) -> Option<(Vec<u8>, (u16, u16))> {
    let mut names = [0u8; LISTDIR_BUF];
    let n = listdir(b"/dev/pci", &mut names);
    if n == usize::MAX {
        return None;
    }
    for name in names[..n].split(|&c| c == b'\n' || c == 0).filter(|s| !s.is_empty() && s[0] != b'.') {
        let dir = path(b"/dev/pci/", name, b"");
        let Some(text) = slurp(&path(&dir, b"/info", b"")) else {
            continue;
        };
        let Some(id) = value(&text, b"id ") else {
            continue;
        };
        let mut parts = id.split(|&c| c == b':');
        let (Some(v), Some(d)) = (parts.next().and_then(hex), parts.next().and_then(hex)) else {
            continue;
        };
        if let Some(&id) = ids.iter().find(|&&id| id == (v as u16, d as u16)) {
            return Some((dir, id));
        }
    }
    None
}

impl Pci {
    /// Hold the function at `dir`: memory decoding and bus mastering on,
    /// BAR0 mapped, the interrupt routed (MSI-X, MSI, or the INTx line of
    /// the aarch64 and riscv64 boards).
    pub fn open(dir: &[u8]) -> Result<Self, &'static str> {
        let ctl = open_flags(&path(dir, b"/ctl", b""), O_RDWR).ok_or("its ctl does not open (driven, or not root)")?;
        let mut p =
            Pci { dir: dir.to_vec(), ctl, regs: core::ptr::null_mut(), regs_len: 0, irq: usize::MAX, msix: false, intx: false };
        if write_fd(ctl, b"enable") != 6 {
            return Err("enable");
        }
        // PCI_CFG_RETRY_TIMEOUT (0x41) to 0, so the device's retries do
        // not interfere with C3 CPU state (iwx_attach).
        let config = open_flags(&path(dir, b"/config", b""), O_RDWR).ok_or("config")?;
        let ok = write_at(config, &[0], 0x41) == 1;
        close(config);
        if !ok {
            return Err("config");
        }
        let text = p.ctl_text().ok_or("ctl")?;
        let size = value(&text, b"bar0 ").and_then(|v| v.split(|&c| c == b' ').nth(1)).and_then(dec).ok_or("no BAR0")?;
        let bar = open_flags(&path(dir, b"/bar0", b""), O_RDWR).ok_or("bar0")?;
        let regs = mmap(size, PROT_READ | PROT_WRITE, MAP_SHARED, bar, 0);
        close(bar);
        p.regs = regs.ok_or("mmap bar0")?;
        p.regs_len = size;
        if write_fd(ctl, b"irq") != 3 {
            return Err("no interrupt");
        }
        let text = p.ctl_text().ok_or("ctl")?;
        (p.msix, p.intx) = match value(&text, b"irq ") {
            Some(b"msix") => (true, false),
            Some(b"msi") => (false, false),
            Some(b"intx") => (false, true),
            _ => return Err("no interrupt"),
        };
        p.irq = open_flags(&path(dir, b"/irq", b""), O_RDWR).ok_or("irq")?;
        // A line starts masked: open it (a write of a byte; the kernel
        // passes a write of none to no file).
        if p.intx && write_fd(p.irq, b"\0") != 1 {
            return Err("irq unmask");
        }
        Ok(p)
    }

    /// The fd that polls ready when the interrupt came.
    pub fn irq_fd(&self) -> usize {
        self.irq
    }

    fn ctl_text(&self) -> Option<Vec<u8>> {
        let mut buf = [0u8; 1024];
        let n = read_at(self.ctl, &mut buf, 0);
        (n != usize::MAX).then(|| buf[..n].to_vec())
    }

    /// The `dmaN` buffers listed.
    fn dma_indexes(&self) -> Vec<usize> {
        let text = self.ctl_text().unwrap_or_default();
        text.split(|&c| c == b'\n').filter_map(|l| l.strip_prefix(b"dma")?.split(|&c| c == b' ').next().and_then(dec)).collect()
    }
}

impl Drop for Pci {
    fn drop(&mut self) {
        if self.irq != usize::MAX {
            close(self.irq);
        }
        // Bus mastering off: the device stops reaching memory.
        close(self.ctl);
    }
}

impl Bus for Pci {
    fn read32(&mut self, off: u32) -> u32 {
        assert!((off as usize) + 4 <= self.regs_len);
        // SAFETY: inside BAR0, mapped uncached by `open`.
        unsafe { core::ptr::read_volatile(self.regs.add(off as usize) as *const u32) }
    }

    fn write32(&mut self, off: u32, val: u32) {
        assert!((off as usize) + 4 <= self.regs_len);
        // SAFETY: as in read32.
        unsafe { core::ptr::write_volatile(self.regs.add(off as usize) as *mut u32, val) }
    }

    fn write8(&mut self, off: u32, val: u8) {
        assert!((off as usize) < self.regs_len);
        // SAFETY: as in read32.
        unsafe { core::ptr::write_volatile(self.regs.add(off as usize), val) }
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

impl Device for Pci {
    fn dma(&mut self, len: usize) -> Option<Dma> {
        let before = self.dma_indexes();
        let mut cmd = Vec::from(b"dma " as &[u8]);
        number(&mut cmd, len.div_ceil(4096));
        if write_fd(self.ctl, &cmd) != cmd.len() {
            return None;
        }
        let i = self.dma_indexes().into_iter().find(|i| !before.contains(i))?;
        let text = self.ctl_text()?;
        let mut name = Vec::from(b"dma" as &[u8]);
        number(&mut name, i);
        name.push(b' ');
        let phys = value(&text, &name).and_then(|v| v.split(|&c| c == b' ').next()).and_then(phys)?;
        name.pop();
        let fd = open_flags(&path(&self.dir, b"/", &name), O_RDWR)?;
        let ptr = mmap(len, PROT_READ | PROT_WRITE, MAP_SHARED, fd, 0);
        close(fd);
        // SAFETY: the buffer is the device's and ours alone while we hold
        // ctl; the mapping is never undone.
        Some(unsafe { Dma::new(ptr?, len, phys) })
    }

    fn wait_irq(&mut self, timeout_us: u64) -> bool {
        let ms = timeout_us.div_ceil(1000).min(i32::MAX as u64) as i32;
        let mut fds = [PollFd { fd: self.irq as i32, events: POLLIN, revents: 0 }];
        if poll(&mut fds, ms) != 1 {
            return false;
        }
        let mut count = [0u8; 8];
        read(self.irq, &mut count) == 8
    }

    fn now_us(&mut self) -> u64 {
        monotonic_ns() / 1000
    }

    /// The line open again: a write of a byte (the kernel passes a write
    /// of none to no file). A message interrupt needs nothing.
    fn irq_done(&mut self) {
        if self.intx {
            write_fd(self.irq, b"\0");
        }
    }
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
    let mut buf = [0u8; 512];
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

fn dec(s: &[u8]) -> Option<usize> {
    core::str::from_utf8(s).ok()?.parse().ok()
}

/// `0x...` as `ctl` writes physical addresses.
fn phys(s: &[u8]) -> Option<u64> {
    hex(s.strip_prefix(b"0x")?)
}

fn number(out: &mut Vec<u8>, mut v: usize) {
    let mut d = [0u8; 20];
    let mut i = d.len();
    loop {
        i -= 1;
        d[i] = b'0' + (v % 10) as u8;
        v /= 10;
        if v == 0 {
            break;
        }
    }
    out.extend_from_slice(&d[i..]);
}
