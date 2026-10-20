//! DMA per device (`KernelApi::dma_attach`, `dma_alloc`, `dma_sync`): which
//! memory a device reaches, the address it uses for it, and whether its DMA
//! sees the CPU's caches.
//!
//! The device tree says so (`dt::pci_dma`, `dt::mmio_dma`: `dma-ranges`,
//! `dma-coherent`). On aarch64 a device not known to be coherent is taken
//! for not, as every aarch64 driver did before: one the tree does not mark
//! (the Arm convention: a Raspberry Pi 4's PCIe is not, QEMU `virt` marks
//! its devices coherent), and one no tree describes (an ACPI-only board).
//! On riscv64 and x86_64 DMA is coherent. A device no `dma-ranges` limit
//! reaches all of memory one-to-one. `dma=noncoherent` on the kernel
//! command line takes every device on aarch64 for non-coherent, to try the
//! cache maintenance on a board whose DMA is coherent.

use alloc::vec::Vec;

use myos_abi::{DMA_DEVICE_MMIO, DMA_DEVICE_PCI, DMA_FROM_DEVICE, DMA_TO_DEVICE, DmaDevice};
use spin::Mutex;

use crate::dt::{DMA_WINDOWS, DmaWindow};

/// What the kernel knows of an attached device.
#[derive(Clone, Copy)]
struct Dma {
    dev: DmaDevice,
    /// The memory it reaches, the first window the first choice.
    windows: [Option<DmaWindow>; DMA_WINDOWS],
    coherent: bool,
}

/// Devices attached, a handle being an index: modules attach theirs once
/// each (the same device gets its handle back).
static TABLE: Mutex<Vec<Dma>> = Mutex::new(Vec::new());

/// Run `f` on [`TABLE`], held with interrupts off on this CPU: `dma_sync`
/// runs in interrupt handlers (xhci's event ring), which must not spin on
/// a lock the code they interrupted holds.
fn with_table<R>(f: impl FnOnce(&mut Vec<Dma>) -> R) -> R {
    let flags = crate::arch::irq_save();
    crate::arch::irq_off();
    let r = f(&mut TABLE.lock());
    crate::arch::irq_restore(flags);
    r
}

fn forced_noncoherent() -> bool {
    cfg!(target_arch = "aarch64") && crate::limine_boot::cmdline().split_whitespace().any(|w| w == "dma=noncoherent")
}

fn resolve(dev: DmaDevice) -> Option<Dma> {
    let described = match dev.kind {
        DMA_DEVICE_PCI => crate::dt::pci_dma(),
        DMA_DEVICE_MMIO => crate::dt::mmio_dma(dev.phys),
        _ => return None,
    };
    let mut windows = [None; DMA_WINDOWS];
    windows[0] = Some(DmaWindow { bus: 0, cpu: 0, size: u64::MAX });
    let mut said = None;
    if let Some(d) = described {
        windows = d.windows;
        said = d.coherent;
    }
    // riscv64imac has no cache maintenance instructions: its DMA is
    // coherent whatever a tree says.
    let coherent = !cfg!(target_arch = "aarch64") || said == Some(true) && !forced_noncoherent();
    Some(Dma { dev, windows, coherent })
}

/// The handle of `dev`, or -1 (not a device kind there is).
pub fn attach(dev: DmaDevice) -> i32 {
    if let Some(i) = with_table(|t| t.iter().position(|d| d.dev == dev)) {
        return i as i32;
    }
    // Resolved unlocked (a walk of the device tree); two modules attaching
    // the same device at once keep the first.
    let Some(dma) = resolve(dev) else {
        return -1;
    };
    with_table(|t| match t.iter().position(|d| d.dev == dev) {
        Some(i) => i as i32,
        None => {
            t.push(dma);
            t.len() as i32 - 1
        }
    })
}

fn get(handle: i32) -> Option<Dma> {
    let i = usize::try_from(handle).ok()?;
    with_table(|t| t.get(i).copied())
}

/// `n` zeroed, contiguous pages the device reaches, from its first window
/// that has them: `(bus address, va)`. Zeroed through the cache: for a
/// non-coherent device the zeros are written back here, before they could
/// be evicted over what it writes.
pub fn alloc(handle: i32, n: usize) -> Option<(u64, *mut u8)> {
    let d = get(handle)?;
    let (w, (phys, va)) = d.windows.iter().flatten().find_map(|w| {
        crate::mm::alloc_contiguous_pages(n, (w.cpu, w.cpu.saturating_add(w.size))).map(|got| (w, got))
    })?;
    if !d.coherent {
        cache::flush(va as usize, n * crate::mm::PAGE as usize);
        cache::barrier();
    }
    Some((phys - w.cpu + w.bus, va))
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
    for d in with_table(|t| t.clone()) {
        let _ = match d.dev.kind {
            DMA_DEVICE_PCI => write!(s, "dma pci {:02x}:{:02x}.{}", d.dev.bus, d.dev.slot, d.dev.func),
            _ => write!(s, "dma mmio 0x{:x}", d.dev.phys),
        };
        for w in d.windows.iter().flatten().filter(|w| w.size != u64::MAX) {
            let _ = write!(s, " 0x{:x} 0x{:x}", w.cpu, w.size);
            if w.bus != w.cpu {
                let _ = write!(s, " bus 0x{:x}", w.bus);
            }
        }
        let _ = writeln!(s, " {}", if d.coherent { "coherent" } else { "noncoherent" });
    }
    s
}

#[cfg(target_arch = "aarch64")]
pub(crate) mod cache {
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
pub(crate) mod cache {
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
