//! DMA per device (`KernelApi::dma_attach`, `dma_alloc`, `dma_sync`): which
//! memory a device reaches, the address it uses for it, and whether its DMA
//! sees the CPU's caches.
//!
//! The device tree says so (`dt::pci_dma`, `dt::mmio_dma`: `dma-ranges`,
//! `dma-coherent`). On aarch64 a device the tree does not mark coherent is
//! not (the Arm convention: a Raspberry Pi 4's PCIe is not, QEMU `virt`
//! marks its devices coherent); on riscv64 and x86_64 DMA is coherent.
//! A device the tree does not describe (x86_64, an ACPI-only board) reaches
//! all of memory one-to-one, coherently. `dma=noncoherent` on the kernel
//! command line takes every device on aarch64 for non-coherent, to try the
//! cache maintenance on a board whose DMA is coherent.

use myos_abi::{DMA_DEVICE_MMIO, DMA_DEVICE_PCI, DMA_FROM_DEVICE, DMA_TO_DEVICE, DmaDevice};
use spin::Mutex;

/// What the kernel knows of an attached device.
#[derive(Clone, Copy)]
struct Dma {
    dev: DmaDevice,
    /// CPU addresses `[cpu, cpu + size)` the device reaches, addressed from
    /// `bus` up.
    bus: u64,
    cpu: u64,
    size: u64,
    coherent: bool,
}

/// Devices attached: modules attach theirs once each (the same device gets
/// its handle back).
const MAX: usize = 64;

static TABLE: Mutex<[Option<Dma>; MAX]> = Mutex::new([None; MAX]);

fn forced_noncoherent() -> bool {
    cfg!(target_arch = "aarch64") && crate::limine_boot::cmdline().split_whitespace().any(|w| w == "dma=noncoherent")
}

fn resolve(dev: DmaDevice) -> Option<Dma> {
    let window = match dev.kind {
        DMA_DEVICE_PCI => crate::dt::pci_dma(),
        DMA_DEVICE_MMIO => crate::dt::mmio_dma(dev.phys),
        _ => return None,
    };
    let (bus, cpu, size, coherent) = match window {
        // riscv64imac has no cache maintenance instructions: its DMA is
        // coherent whatever a tree says.
        Some(w) if cfg!(target_arch = "aarch64") => (w.bus, w.cpu, w.size, w.coherent.unwrap_or(false)),
        Some(w) => (w.bus, w.cpu, w.size, true),
        None => (0, 0, u64::MAX, true),
    };
    Some(Dma { dev, bus, cpu, size, coherent: coherent && !forced_noncoherent() })
}

/// The handle of `dev`: an index into the table, or -1 (full, or not a
/// device kind there is).
pub fn attach(dev: DmaDevice) -> i32 {
    let mut table = TABLE.lock();
    if let Some(i) = table.iter().position(|d| d.is_some_and(|d| d.dev == dev)) {
        return i as i32;
    }
    let Some(free) = table.iter().position(|d| d.is_none()) else {
        return -1;
    };
    let Some(dma) = resolve(dev) else {
        return -1;
    };
    table[free] = Some(dma);
    free as i32
}

fn get(handle: i32) -> Option<Dma> {
    *TABLE.lock().get(usize::try_from(handle).ok()?)?
}

/// `n` zeroed, contiguous pages the device reaches: `(bus address, va)`.
/// Zeroed through the cache: for a non-coherent device the zeros are
/// written back here, before they could be evicted over what it writes.
pub fn alloc(handle: i32, n: usize) -> Option<(u64, *mut u8)> {
    let d = get(handle)?;
    let (phys, va) = crate::mm::alloc_contiguous_pages(n, (d.cpu, d.cpu.saturating_add(d.size)))?;
    if !d.coherent {
        cache::flush(va as usize, n * crate::mm::PAGE as usize);
        cache::barrier();
    }
    Some((phys - d.cpu + d.bus, va))
}

/// Hand `len` bytes at `va` over to the device (`DMA_TO_DEVICE`) or back
/// (`DMA_FROM_DEVICE`).
pub fn sync(handle: i32, va: *const u8, len: usize, dir: u32) {
    let noncoherent = get(handle).is_some_and(|d| !d.coherent);
    if noncoherent && len != 0 {
        match dir {
            DMA_TO_DEVICE => cache::clean(va as usize, len),
            DMA_FROM_DEVICE => cache::flush(va as usize, len),
            _ => {}
        }
    }
    cache::barrier();
}

/// `/proc/platform`'s DMA lines: one per device attached.
pub fn text() -> alloc::string::String {
    use core::fmt::Write;
    let mut s = alloc::string::String::new();
    for d in TABLE.lock().iter().flatten() {
        let _ = match d.dev.kind {
            DMA_DEVICE_PCI => write!(s, "dma pci {:02x}:{:02x}.{}", d.dev.bus, d.dev.slot, d.dev.func),
            _ => write!(s, "dma mmio 0x{:x}", d.dev.phys),
        };
        if d.size != u64::MAX {
            let _ = write!(s, " 0x{:x} 0x{:x}", d.cpu, d.size);
            if d.bus != d.cpu {
                let _ = write!(s, " bus 0x{:x}", d.bus);
            }
        }
        let _ = writeln!(s, " {}", if d.coherent { "coherent" } else { "noncoherent" });
    }
    s
}

#[cfg(target_arch = "aarch64")]
mod cache {
    use core::arch::asm;

    /// The smallest data cache line (CTR_EL0.DminLine, in words).
    fn line() -> usize {
        let ctr: u64;
        unsafe {
            asm!("mrs {c}, ctr_el0", c = out(reg) ctr, options(nomem, nostack, preserves_flags));
        }
        4 << ((ctr >> 16) & 0xf)
    }

    /// Write the lines of `[va, va + len)` back to memory (`dc cvac`).
    pub fn clean(va: usize, len: usize) {
        let line = line();
        let mut a = va & !(line - 1);
        while a < va + len {
            unsafe {
                asm!("dc cvac, {a}", a = in(reg) a, options(nostack, preserves_flags));
            }
            a += line;
        }
    }

    /// Write back and drop the lines of `[va, va + len)` (`dc civac`): the
    /// next read comes from memory. Not a bare invalidate, which would lose
    /// what the CPU wrote to the lines a buffer shares at its ends.
    pub fn flush(va: usize, len: usize) {
        let line = line();
        let mut a = va & !(line - 1);
        while a < va + len {
            unsafe {
                asm!("dc civac, {a}", a = in(reg) a, options(nostack, preserves_flags));
            }
            a += line;
        }
    }

    /// The maintenance and the CPU's writes complete before what follows
    /// (a doorbell), the device's writes are seen by what follows.
    pub fn barrier() {
        unsafe {
            asm!("dsb sy", options(nostack, preserves_flags));
        }
    }
}

#[cfg(not(target_arch = "aarch64"))]
mod cache {
    // Coherent DMA only (`resolve`): nothing to clean.
    pub fn clean(_va: usize, _len: usize) {}

    pub fn flush(_va: usize, _len: usize) {}

    pub fn barrier() {
        core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);
        #[cfg(target_arch = "riscv64")]
        unsafe {
            core::arch::asm!("fence rw,rw", options(nostack, preserves_flags));
        }
    }
}
