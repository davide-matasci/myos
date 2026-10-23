//! `/dev/pci`: PCI functions for drivers that run as programs.
//!
//! A directory per function (`BB:SS.F`, the PCI address in hex), endpoints
//! only (no bridges), with:
//!
//! - `info`: what the function is, `id VVVV:DDDD` and `class CC:SS:PP`
//!   (vendor and device; class, subclass and programming interface), and
//!   who drives it (`driver nvme`, `driver pcidev` while a program holds
//!   it; no line when nobody does).
//! - `ctl`: held by one program at a time (the driver; root only), from
//!   its open on, and only on a function no kernel driver drives (its
//!   open is refused then: the PCI claim, `KernelApi::pci_claim`). Reads as lines: each memory BAR (`barN ADDR SIZE`, the
//!   address as the CPUs see it, `KernelApi::pci_bar_phys`: the host
//!   bridge's window moves it on some boards), the
//!   interrupt once routed (`irq msix|msi|intx`), the IOMMU domain the
//!   function is confined to while held (`iommu N`: its DMA reaches the
//!   buffers below and nothing else, `KernelApi::iommu_attach`; no line on
//!   a board without an IOMMU, where it reaches all of memory), whether
//!   the device's DMA sees the caches (`dma coherent`, or `dma
//!   noncoherent`: the buffers are then mapped non-cacheable, so the
//!   program needs no cache maintenance) and each DMA buffer
//!   (`dmaN BUS PAGES`, the address the device uses for it). Writes: `enable` (memory decoding
//!   and bus mastering on), `disable` (off), `irq` (route the function's
//!   interrupt to `irq`), `dma PAGES` (a physically contiguous, zeroed
//!   buffer, the next `dmaN`). Closing it (the driver's exit too) turns
//!   bus mastering and memory decoding off, so the device stops reaching
//!   memory, and gives the DMA buffers back to this module for the next
//!   driver (they are never freed: a mapping of one may outlive the fd).
//! - `config`: the 256 bytes of configuration space, read and written at
//!   any offset (whole dwords underneath).
//! - `barN`: a memory BAR's registers, for `mmap(MAP_SHARED)` (uncached).
//! - `irq`: a read waits for the interrupt and returns how many came since
//!   the last read (8 bytes, little endian); `poll` says one is pending.
//!   A legacy INTx line stays masked (the command register's interrupt
//!   disable) from the interrupt until a write to `irq` (of a byte at
//!   least: the kernel passes an empty one to no file), once the driver
//!   has quietened the device; MSI and MSI-X need no write.
//! - `dmaN`: the buffer, for `mmap(MAP_SHARED)`: cached memory, or
//!   non-cacheable (`MYOS_MMAP_NOCACHE`) for a device whose DMA does not
//!   see the caches.
//!
//! The files besides `info` and `ctl` are only there while `ctl` is held.
//!
//! The module ships in the image but is not loaded at boot: a program that
//! drives a device is as trusted as the kernel on a board without an IOMMU
//! (issue #501), so root loads it, `insmod /lib/modules/pcidev`.

#![no_std]
#![no_main]
#![deny(unsafe_op_in_unsafe_fn)]

use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};

use myos_abi::{
    DmaDevice, status_ok, status_warn, ApiCell, KernelApi, Lock, ModuleVfsOps, VfsStatInfo, ABI_VERSION, MYOS_IRQ_INTX,
    MYOS_IRQ_MSI, MYOS_MMAP_NOCACHE, MYOS_MMAP_UNCACHED, MYOS_POLLIN, MYOS_READ_WAIT, PCI_OWNER_MAX,
    DMA_TO_DEVICE,
};

const S_IFDIR: u32 = 0o040000;
const S_IFREG: u32 = 0o100000;
const PAGE: usize = 4096;
/// Functions listed (the first found, bus by bus).
const MAX_DEV: usize = 32;
/// Buses scanned, as `pci_enum` does.
const MAX_BUS: u8 = 32;
/// DMA buffers per function.
const MAX_DMA: usize = 16;
/// The configuration space the files reach.
const CONFIG_LEN: usize = 256;
/// Command register bits.
const CMD_MEM: u32 = 1 << 1;
const CMD_MASTER: u32 = 1 << 2;
const CMD_INTX_OFF: u32 = 1 << 10;
/// The owner of the functions a program drives ([`KernelApi::pci_claim`]).
const OWNER: &str = "pcidev";

static API: ApiCell = ApiCell::new();

fn api() -> &'static KernelApi {
    API.get()
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Irq {
    /// Not routed yet (`irq` not written to `ctl`).
    None,
    MsiX,
    Msi,
    IntX,
}

#[derive(Clone, Copy)]
struct Dma {
    /// The address the device uses for it.
    bus: u64,
    /// The kernel's mapping of it (the HHDM: its physical address too).
    va: usize,
    pages: usize,
    /// The holder's (a `dmaN` file); else free for the next `dma` asked.
    used: bool,
}

#[derive(Clone, Copy)]
struct Dev {
    bus: u8,
    slot: u8,
    func: u8,
    id: u32,
    class: u32,
    /// `ctl` is open.
    held: bool,
    /// Memory BARs (`(cpu address, size)`, size 0: none), probed at the
    /// first hold.
    bars: [(u64, u64); 6],
    probed: bool,
    irq: Irq,
    /// The IOMMU domain while held, -1 without one.
    iommu: i32,
    /// The function's DMA handle (`KernelApi::dma_attach`) from the first
    /// hold on, -1 before, and whether its DMA sees the caches.
    dma_dev: i32,
    coherent: bool,
    dma: [Dma; MAX_DMA],
}

const NO_DMA: Dma = Dma { bus: 0, va: 0, pages: 0, used: false };

static DEVS: Lock<[Option<Dev>; MAX_DEV]> = Lock::new([None; MAX_DEV]);
/// Per function, for the interrupt handler (no lock there): interrupts
/// not read yet, all of them (`poll`'s change count), and its address
/// (`bus << 16 | slot << 8 | func`, with bit 31 for an INTx line to mask).
static PENDING: [AtomicU64; MAX_DEV] = [const { AtomicU64::new(0) }; MAX_DEV];
static TOTAL: [AtomicU64; MAX_DEV] = [const { AtomicU64::new(0) }; MAX_DEV];
static ADDR: [AtomicU32; MAX_DEV] = [const { AtomicU32::new(0) }; MAX_DEV];
const ADDR_INTX: u32 = 1 << 31;

#[derive(Clone, Copy, PartialEq, Eq)]
enum File {
    Info,
    Ctl,
    Config,
    Irq,
    Bar(usize),
    Dma(usize),
}

enum Node {
    Root,
    Dir(usize),
    File(usize, File),
}

/// `BB:SS.F` for a function.
fn dev_name(d: &Dev) -> [u8; 7] {
    let hex = |v: u8| if v < 10 { b'0' + v } else { b'a' + v - 10 };
    [hex(d.bus >> 4), hex(d.bus & 15), b':', hex(d.slot >> 4), hex(d.slot & 15), b'.', hex(d.func)]
}

fn parse_file(s: &[u8], d: &Dev) -> Option<File> {
    let index = |rest: &[u8]| -> Option<usize> {
        if rest.is_empty() || rest.len() > 2 || !rest.iter().all(u8::is_ascii_digit) {
            return None;
        }
        Some(rest.iter().fold(0, |n, c| n * 10 + (c - b'0') as usize))
    };
    let file = match s {
        b"info" => File::Info,
        b"ctl" => File::Ctl,
        b"config" => File::Config,
        b"irq" => File::Irq,
        _ if s.starts_with(b"bar") => File::Bar(index(&s[3..]).filter(|&i| i < 6)?),
        _ if s.starts_with(b"dma") => File::Dma(index(&s[3..]).filter(|&i| i < MAX_DMA)?),
        _ => return None,
    };
    // Only `info` and `ctl` while nobody drives the function.
    let there = match file {
        File::Info | File::Ctl => true,
        _ if !d.held => false,
        File::Config | File::Irq => true,
        File::Bar(i) => d.bars[i].1 != 0,
        File::Dma(i) => d.dma[i].used,
    };
    there.then_some(file)
}

fn node(devs: &[Option<Dev>; MAX_DEV], path: *const u8, len: usize) -> Option<Node> {
    let path = if path.is_null() || len == 0 { &[][..] } else { unsafe { core::slice::from_raw_parts(path, len) } };
    let path = path.strip_suffix(b"/").unwrap_or(path);
    if path.is_empty() || path == b"." {
        return Some(Node::Root);
    }
    let (dir, file) = match path.iter().position(|&c| c == b'/') {
        Some(i) => (&path[..i], Some(&path[i + 1..])),
        None => (path, None),
    };
    let i = devs.iter().position(|d| d.as_ref().is_some_and(|d| dev_name(d) == dir))?;
    let d = devs[i].as_ref()?;
    match file {
        None => Some(Node::Dir(i)),
        Some(f) => Some(Node::File(i, parse_file(f, d)?)),
    }
}

/// A text buffer for `ctl` and the listings.
struct Text {
    buf: [u8; 1024],
    len: usize,
}

impl Text {
    fn new() -> Self {
        Self { buf: [0; 1024], len: 0 }
    }

    fn push(&mut self, s: &[u8]) {
        let n = s.len().min(self.buf.len() - self.len);
        self.buf[self.len..self.len + n].copy_from_slice(&s[..n]);
        self.len += n;
    }

    fn dec(&mut self, mut v: u64) {
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
        self.push(&d[i..]);
    }

    /// `v` in `digits` hex digits (at least: all of them for a wider one).
    fn hex(&mut self, v: u64, digits: usize) {
        let mut d = [0u8; 16];
        let mut n = 0;
        let mut v2 = v;
        while v2 != 0 || n < digits.max(1) {
            let x = (v2 & 15) as u8;
            d[15 - n] = if x < 10 { b'0' + x } else { b'a' + x - 10 };
            v2 >>= 4;
            n += 1;
            if n == 16 {
                break;
            }
        }
        self.push(&d[16 - n..]);
    }

    fn bytes(&self) -> &[u8] {
        &self.buf[..self.len]
    }
}

fn info_text(d: &Dev) -> Text {
    let mut t = Text::new();
    t.push(b"id ");
    t.hex(u64::from(d.id & 0xFFFF), 4);
    t.push(b":");
    t.hex(u64::from(d.id >> 16), 4);
    t.push(b"\nclass ");
    t.hex(u64::from(d.class >> 24), 2);
    t.push(b":");
    t.hex(u64::from((d.class >> 16) & 0xFF), 2);
    t.push(b":");
    t.hex(u64::from((d.class >> 8) & 0xFF), 2);
    t.push(b"\n");
    let mut owner = [0u8; PCI_OWNER_MAX];
    let owner = api().pci_owner(d.bus, d.slot, d.func, &mut owner);
    if !owner.is_empty() {
        t.push(b"driver ");
        t.push(owner);
        t.push(b"\n");
    }
    t
}

fn ctl_text(d: &Dev) -> Text {
    let mut t = Text::new();
    for (i, &(phys, size)) in d.bars.iter().enumerate() {
        if size != 0 {
            t.push(b"bar");
            t.dec(i as u64);
            t.push(b" 0x");
            t.hex(phys, 1);
            t.push(b" ");
            t.dec(size);
            t.push(b"\n");
        }
    }
    let irq: &[u8] = match d.irq {
        Irq::None => b"",
        Irq::MsiX => b"irq msix\n",
        Irq::Msi => b"irq msi\n",
        Irq::IntX => b"irq intx\n",
    };
    t.push(irq);
    if d.iommu >= 0 {
        t.push(b"iommu ");
        t.dec(d.iommu as u64);
        t.push(b"\n");
    }
    t.push(if d.coherent { b"dma coherent\n" } else { b"dma noncoherent\n" });
    for (i, b) in d.dma.iter().enumerate() {
        if b.used {
            t.push(b"dma");
            t.dec(i as u64);
            t.push(b" 0x");
            t.hex(b.bus, 1);
            t.push(b" ");
            t.dec(b.pages as u64);
            t.push(b"\n");
        }
    }
    t
}

/// The function's memory BARs, each at the address the CPUs see it
/// (`pci_bar_phys`: assigned when the firmware left it unassigned, the
/// host bridge's window applied).
fn probe_bars(d: &mut Dev) {
    let api = api();
    let mut i = 0;
    while i < 6 {
        let off = 0x10 + 4 * i as u8;
        let lo = api.pci_cfg_read32(d.bus, d.slot, d.func, off);
        let is64 = lo & 0x7 == 0x4;
        let (mut cpu, mut size) = (0u64, 0u64);
        if lo & 1 == 0 && api.pci_bar_phys(d.bus, d.slot, d.func, i as u8, &mut cpu, &mut size) == 0 {
            d.bars[i] = (cpu, size);
        }
        i += if is64 { 2 } else { 1 };
    }
    d.probed = true;
}

fn command(d: &Dev, set: u32, clear: u32) {
    let api = api();
    let v = api.pci_cfg_read32(d.bus, d.slot, d.func, 4) & 0xFFFF;
    api.pci_cfg_write32(d.bus, d.slot, d.func, 4, (v | set) & !clear);
}

/// The function's interrupt: count it, mask an INTx line until the driver
/// says it is quiet (a write to `irq`), wake the reader.
unsafe extern "C" fn irq_handler(ctx: *mut core::ffi::c_void) {
    let i = ctx as usize;
    if i >= MAX_DEV {
        return;
    }
    let addr = ADDR[i].load(Ordering::Relaxed);
    if addr & ADDR_INTX != 0 {
        let api = api();
        let (bus, slot, func) = ((addr >> 16) as u8, (addr >> 8) as u8, addr as u8);
        // A shared line raised by another device: not ours.
        if api.pci_cfg_read32(bus, slot, func, 4) & (1 << 19) == 0 {
            return;
        }
        let v = api.pci_cfg_read32(bus, slot, func, 4) & 0xFFFF;
        api.pci_cfg_write32(bus, slot, func, 4, v | CMD_INTX_OFF);
    }
    PENDING[i].fetch_add(1, Ordering::Relaxed);
    TOTAL[i].fetch_add(1, Ordering::Relaxed);
    api().wake_any();
}

fn route_irq(i: usize, d: &mut Dev) -> bool {
    if d.irq != Irq::None {
        return true;
    }
    ADDR[i].store(u32::from(d.bus) << 16 | u32::from(d.slot) << 8 | u32::from(d.func), Ordering::Relaxed);
    // INTx starts masked: the handler masks it once it fired, and the
    // driver unmasks (a write to `irq`) when it is ready for one.
    command(d, CMD_INTX_OFF, 0);
    let mut entry = MYOS_IRQ_INTX;
    let rc = api().pci_irq_enable(d.bus, d.slot, d.func, "pcidev", irq_handler, i as *mut _, &mut entry);
    if rc != 0 {
        return false;
    }
    d.irq = match entry {
        MYOS_IRQ_INTX => {
            ADDR[i].fetch_or(ADDR_INTX, Ordering::Relaxed);
            Irq::IntX
        }
        MYOS_IRQ_MSI => Irq::Msi,
        _ => Irq::MsiX,
    };
    // MSI and MSI-X do not use the line.
    if d.irq != Irq::IntX {
        command(d, 0, 0);
    }
    true
}

/// A zeroed buffer of `pages`: a free one big enough, else a new one.
fn dma_get(d: &mut Dev, pages: usize) -> bool {
    let reuse = d.dma.iter().position(|b| !b.used && b.pages >= pages);
    let i = match reuse {
        Some(i) => {
            let b = d.dma[i];
            // SAFETY: dma_alloc returned this buffer's mapping.
            unsafe { core::ptr::write_bytes(b.va as *mut u8, 0, b.pages * PAGE) };
            // The zeros reach a device that does not see the caches.
            if !d.coherent {
                api().dma_sync(d.dma_dev, b.va as *const u8, b.pages * PAGE, DMA_TO_DEVICE);
            }
            i
        }
        None => {
            let Some(i) = d.dma.iter().position(|b| b.pages == 0) else {
                return false;
            };
            let mut bus = 0;
            let va = api().dma_alloc(d.dma_dev, pages, &mut bus);
            if va.is_null() {
                return false;
            }
            d.dma[i] = Dma { bus, va: va as usize, pages, used: false };
            if d.iommu >= 0 && !api().iommu_map(d.iommu, bus, pages * PAGE) {
                return false;
            }
            i
        }
    };
    d.dma[i].used = true;
    true
}

fn ctl_write(i: usize, d: &mut Dev, cmd: &[u8]) -> bool {
    match cmd {
        b"enable" => {
            command(d, CMD_MEM | CMD_MASTER, 0);
            true
        }
        b"disable" => {
            command(d, 0, CMD_MEM | CMD_MASTER);
            true
        }
        b"irq" => route_irq(i, d),
        _ => match cmd.strip_prefix(b"dma ") {
            Some(n) if !n.is_empty() && n.len() <= 6 && n.iter().all(u8::is_ascii_digit) => {
                let pages = n.iter().fold(0usize, |v, c| v * 10 + (c - b'0') as usize);
                pages != 0 && dma_get(d, pages)
            }
            _ => false,
        },
    }
}

/// The driver let go: the device stops reaching memory, its buffers wait
/// for the next driver.
fn let_go(i: usize, d: &mut Dev) {
    command(d, CMD_INTX_OFF, CMD_MEM | CMD_MASTER);
    for b in d.dma.iter_mut() {
        b.used = false;
    }
    PENDING[i].store(0, Ordering::Relaxed);
    d.held = false;
    if d.iommu >= 0 {
        api().iommu_detach(d.bus, d.slot, d.func);
        d.iommu = -1;
    }
    api().pci_release(d.bus, d.slot, d.func, OWNER);
}

fn file_size(d: &Dev, f: File) -> u64 {
    match f {
        File::Info => info_text(d).len as u64,
        File::Ctl => ctl_text(d).len as u64,
        File::Config => CONFIG_LEN as u64,
        File::Irq => 8,
        File::Bar(n) => d.bars[n].1,
        File::Dma(n) => (d.dma[n].pages * PAGE) as u64,
    }
}

unsafe extern "C" fn pci_lookup(_: *const u8, _: usize, _: *mut *const u8, _: *mut usize) -> i32 {
    -1
}

unsafe extern "C" fn pci_stat(path: *const u8, len: usize, out: *mut VfsStatInfo) -> i32 {
    let devs = DEVS.lock();
    let Some(n) = node(&devs, path, len) else {
        return -1;
    };
    let (mode, size, ino) = match n {
        Node::Root => (S_IFDIR | 0o755, 0, 1),
        Node::Dir(i) => (S_IFDIR | 0o755, 0, 64 * (i as u64 + 1)),
        Node::File(i, f) => {
            let d = devs[i].as_ref().unwrap();
            let k = match f {
                File::Info => 0,
                File::Ctl => 1,
                File::Config => 2,
                File::Irq => 3,
                File::Bar(n) => 4 + n as u64,
                File::Dma(n) => 10 + n as u64,
            };
            let mode = if f == File::Info { 0o444 } else { 0o600 };
            (S_IFREG | mode, file_size(d, f), 64 * (i as u64 + 1) + k)
        }
    };
    let dir = matches!(n, Node::Root | Node::Dir(_));
    let ino = ino as u32;
    unsafe { *out = VfsStatInfo { mode, size, ino, nlink: if dir { 2 } else { 1 }, mtime: 0, atime: 0 } };
    0
}

unsafe extern "C" fn pci_listdir(path: *const u8, len: usize, buf: *mut u8, cap: usize, out_len: *mut usize) -> i32 {
    let devs = DEVS.lock();
    let mut t = Text::new();
    match node(&devs, path, len) {
        Some(Node::Root) => {
            for d in devs.iter().flatten() {
                t.push(&dev_name(d));
                t.push(b"\n");
            }
        }
        Some(Node::Dir(i)) => {
            let d = devs[i].as_ref().unwrap();
            t.push(b"info\nctl\n");
            if d.held {
                t.push(b"config\nirq\n");
                for n in 0..6 {
                    if d.bars[n].1 != 0 {
                        t.push(b"bar");
                        t.dec(n as u64);
                        t.push(b"\n");
                    }
                }
                for n in 0..MAX_DMA {
                    if d.dma[n].used {
                        t.push(b"dma");
                        t.dec(n as u64);
                        t.push(b"\n");
                    }
                }
            }
        }
        _ => return -1,
    }
    if t.len > cap {
        return -1;
    }
    unsafe {
        core::ptr::copy_nonoverlapping(t.buf.as_ptr(), buf, t.len);
        *out_len = t.len;
    }
    0
}

/// `ctl`: one holder, root.
unsafe extern "C" fn pci_open(path: *const u8, len: usize, _out: *mut u8, _out_cap: usize) -> i32 {
    let mut devs = DEVS.lock();
    let Some(Node::File(i, f)) = node(&devs, path, len) else {
        return 0;
    };
    if f != File::Ctl {
        return 0;
    }
    let d = devs[i].as_mut().unwrap();
    // A function a kernel driver drives is the driver's: closing ctl turns
    // the device off.
    if d.held || api().current_uid() != 0 || !api().pci_claim(d.bus, d.slot, d.func, OWNER) {
        return -1;
    }
    if !d.probed {
        probe_bars(d);
    }
    if d.dma_dev < 0 {
        d.dma_dev = api().dma_attach(&DmaDevice::pci(d.bus, d.slot, d.func));
        d.coherent = api().dma_coherent(d.dma_dev).unwrap_or(true);
    }
    // Confined to its own domain where there is an IOMMU: the buffers kept
    // from the last driver go in, nothing else.
    d.iommu = api().iommu_attach(d.bus, d.slot, d.func).unwrap_or(-1);
    if d.iommu >= 0
        && !d.dma.iter().filter(|b| b.pages != 0).all(|b| api().iommu_map(d.iommu, b.bus, b.pages * PAGE))
    {
        api().iommu_detach(d.bus, d.slot, d.func);
        d.iommu = -1;
        api().pci_release(d.bus, d.slot, d.func, OWNER);
        return -1;
    }
    d.held = true;
    0
}

unsafe extern "C" fn pci_release(path: *const u8, len: usize) -> i32 {
    let mut devs = DEVS.lock();
    if let Some(Node::File(i, File::Ctl)) = node(&devs, path, len) {
        let d = devs[i].as_mut().unwrap();
        if d.held {
            let_go(i, d);
        }
    }
    0
}

unsafe extern "C" fn pci_read(path: *const u8, len: usize, pos: usize, buf: *mut u8, cap: usize) -> i32 {
    let devs = DEVS.lock();
    let Some(Node::File(i, f)) = node(&devs, path, len) else {
        return -1;
    };
    let d = devs[i].as_ref().unwrap();
    let out = |src: &[u8]| {
        let n = cap.min(src.len().saturating_sub(pos));
        if n != 0 {
            unsafe { core::ptr::copy_nonoverlapping(src[pos..].as_ptr(), buf, n) };
        }
        n as i32
    };
    match f {
        File::Info => out(info_text(d).bytes()),
        File::Ctl => out(ctl_text(d).bytes()),
        File::Config => {
            let mut cfg = [0u8; CONFIG_LEN];
            let (start, end) = (pos.min(CONFIG_LEN) & !3, (pos + cap).min(CONFIG_LEN));
            let mut off = start;
            while off < end {
                let v = api().pci_cfg_read32(d.bus, d.slot, d.func, off as u8);
                cfg[off..off + 4].copy_from_slice(&v.to_le_bytes());
                off += 4;
            }
            out(&cfg)
        }
        File::Irq => {
            let n = PENDING[i].swap(0, Ordering::Relaxed);
            if n == 0 {
                return MYOS_READ_WAIT;
            }
            if cap < 8 {
                PENDING[i].fetch_add(n, Ordering::Relaxed);
                return -1;
            }
            unsafe { core::ptr::copy_nonoverlapping(n.to_le_bytes().as_ptr(), buf, 8) };
            8
        }
        File::Info | File::Bar(_) | File::Dma(_) => -1,
    }
}

unsafe extern "C" fn pci_write(path: *const u8, len: usize, pos: usize, buf: *const u8, n: usize) -> i32 {
    let mut devs = DEVS.lock();
    let Some(Node::File(i, f)) = node(&devs, path, len) else {
        return -1;
    };
    let d = devs[i].as_mut().unwrap();
    let data = unsafe { core::slice::from_raw_parts(buf, n) };
    match f {
        File::Ctl => {
            if ctl_write(i, d, data.trim_ascii()) {
                n as i32
            } else {
                -1
            }
        }
        File::Config => {
            let api = api();
            let end = (pos + n).min(CONFIG_LEN);
            let mut at = pos;
            while at < end {
                // Read-modify-write of the dword holding `at`.
                let off = at & !3;
                let mut v = api.pci_cfg_read32(d.bus, d.slot, d.func, off as u8).to_le_bytes();
                while at < end && at < off + 4 {
                    v[at - off] = data[at - pos];
                    at += 1;
                }
                api.pci_cfg_write32(d.bus, d.slot, d.func, off as u8, u32::from_le_bytes(v));
            }
            (end.saturating_sub(pos)) as i32
        }
        File::Irq => {
            // The driver has quietened the device: the line may fire again.
            if d.irq == Irq::IntX {
                command(d, 0, CMD_INTX_OFF);
            }
            n as i32
        }
        File::Info | File::Bar(_) | File::Dma(_) => -1,
    }
}

unsafe extern "C" fn pci_mmap(path: *const u8, len: usize, offset: usize, attrs: *mut u32) -> u64 {
    let devs = DEVS.lock();
    let Some(Node::File(i, f)) = node(&devs, path, len) else {
        return 0;
    };
    let d = devs[i].as_ref().unwrap();
    if offset % PAGE != 0 {
        return 0;
    }
    match f {
        File::Bar(n) => {
            let (phys, size) = d.bars[n];
            // A BAR smaller than a page is mapped whole (its page).
            if phys % PAGE as u64 != 0 || offset as u64 >= size.next_multiple_of(PAGE as u64) {
                return 0;
            }
            unsafe { *attrs = MYOS_MMAP_UNCACHED };
            phys + offset as u64
        }
        File::Dma(n) => {
            let b = d.dma[n];
            if offset >= b.pages * PAGE {
                return 0;
            }
            // A device that does not see the caches reads what the program
            // writes only through a non-cacheable mapping.
            unsafe { *attrs = if d.coherent { 0 } else { MYOS_MMAP_NOCACHE } };
            b.va as u64 - api().hhdm_offset() + offset as u64
        }
        _ => 0,
    }
}

unsafe extern "C" fn pci_poll(path: *const u8, len: usize, changes: *mut u64) -> u32 {
    let devs = DEVS.lock();
    let Some(Node::File(i, File::Irq)) = node(&devs, path, len) else {
        return 0;
    };
    if !changes.is_null() {
        unsafe { *changes = TOTAL[i].load(Ordering::Relaxed) };
    }
    if PENDING[i].load(Ordering::Relaxed) != 0 { MYOS_POLLIN } else { 0 }
}

/// The endpoints on the first [`MAX_BUS`] buses (header type 0).
fn scan(devs: &mut [Option<Dev>; MAX_DEV]) -> usize {
    let api = api();
    let mut n = 0;
    for bus in 0..MAX_BUS {
        for slot in 0..32u8 {
            if api.pci_cfg_read32(bus, slot, 0, 0) & 0xFFFF == 0xFFFF {
                continue;
            }
            let multi = api.pci_cfg_read32(bus, slot, 0, 0x0C) & (0x80 << 16) != 0;
            for func in 0..if multi { 8u8 } else { 1 } {
                let id = api.pci_cfg_read32(bus, slot, func, 0);
                if id & 0xFFFF == 0xFFFF || (api.pci_cfg_read32(bus, slot, func, 0x0C) >> 16) & 0x7F != 0 {
                    continue;
                }
                if n == MAX_DEV {
                    return n;
                }
                devs[n] = Some(Dev {
                    bus,
                    slot,
                    func,
                    id,
                    class: api.pci_cfg_read32(bus, slot, func, 8),
                    held: false,
                    bars: [(0, 0); 6],
                    probed: false,
                    irq: Irq::None,
                    iommu: -1,
                    dma_dev: -1,
                    coherent: true,
                    dma: [NO_DMA; MAX_DMA],
                });
                n += 1;
            }
        }
    }
    n
}

#[inline(never)]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn module_init(api_ptr: *const KernelApi) -> i32 {
    let Some(api) = (unsafe { api_ptr.as_ref() }) else {
        return -1;
    };
    if api.abi_version != ABI_VERSION {
        return -2;
    }
    unsafe { API.set(api_ptr) };
    scan(&mut DEVS.lock());
    // Built here, not in a static: aarch64 ET_EXEC modules do not relocate
    // fn pointers in .rodata (see netfs).
    let ops = ModuleVfsOps {
        lookup: pci_lookup,
        stat: pci_stat,
        listdir: pci_listdir,
        register: None,
        read: Some(pci_read),
        write: Some(pci_write),
        create: None,
        truncate: None,
        mkdir: None,
        rmdir: None,
        unlink: None,
        rename: None,
        symlink: None,
        readlink: None,
        release: Some(pci_release),
        mmap: Some(pci_mmap),
        poll: Some(pci_poll),
        open: Some(pci_open),
        set_times: None,
        unmount: None,
        unlink_keep: None,
        read_ino: None,
        write_ino: None,
        stat_ino: None,
        forget_ino: None,
        set_size: None,
        set_size_ino: None,
        file_id: None,
        set_times_ino: None,
        statfs: None,
    };
    if api.vfs_mount("pci", "dev/pci", &ops) != 0 {
        status_warn(api, "pcidev: /dev/pci");
        return -3;
    }
    status_ok(api, "pcidev");
    0
}

#[inline(never)]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn module_exit() {}

// So `cargo build --bin pcidev` links. The kernel never jumps here.
#[unsafe(no_mangle)]
pub extern "C" fn _start() -> ! {
    loop {}
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    loop {}
}
