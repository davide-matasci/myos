//! Device interrupt registry: one handler per interrupt number, shared by
//! the three architectures' second-level dispatch.
//!
//! * x86_64: MSI-X and MSI messages and I/O APIC pins (the legacy ISA
//!   interrupts, and PCI INTx lines where the firmware's `_PRT` routes
//!   them) land on LAPIC vectors `DEVICE_VECTOR_BASE..`; the IDT stubs call
//!   [`dispatch`] with the vector.
//! * aarch64: PCI INTx lines and the board's devices are GIC SPIs;
//!   `aarch64_irq_handler` passes any non-timer, non-SGI INTID here.
//! * riscv64: PCI INTx lines and the board's devices are PLIC sources; the
//!   supervisor external interrupt claims from the PLIC and passes the
//!   source id here.
//! * an interrupt domain's lines (`irqchip::DOMAIN_FIRST` and up, a
//!   secondary controller a module drives): the module's handler of the
//!   controller's own interrupt passes each pending one here
//!   (`KernelApi::irq_dispatch`).
//!
//! Handlers run in interrupt context on the CPU that took the interrupt
//! (CPU 0: every interrupt is routed to the BSP). They ack the device
//! (virtio ISR read) and typically `task::wake*` a sleeper. An INTx line is
//! shared by the PCI slots the swizzle puts on it (QEMU `virt` has four
//! for every slot): every handler on it runs, and each one's device says
//! whether it was the one.
//!
//! Modules get this through `KernelApi::pci_irq_enable`, which also picks
//! the delivery mechanism per arch (`pci_irq_setup`), and
//! `KernelApi::irq_enable` for a board device's own interrupt ([`enable`]).
//! A PCI function's interrupt is given back with
//! `KernelApi::pci_irq_disable` ([`pci_irq_disable`]): its handler's slot,
//! and once its last handler is gone what the arch took for it (x86_64:
//! the vector, the IOMMU's remapping entry).

use core::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use spin::Mutex;

pub type Handler = unsafe extern "C" fn(ctx: *mut core::ffi::c_void);

/// Handlers on every arch together: as many as x86_64 has device vectors
/// (a shared INTx line on aarch64 and riscv64 takes one per device on it).
/// `dispatch` copies the table onto the interrupted stack, 2.5 KiB.
const MAX_HANDLERS: usize = 64;

#[derive(Clone, Copy)]
struct Entry {
    irq: u32,
    /// The PCI function whose interrupt it is (`pci_irq_enable`).
    pci: Option<(u8, u8, u8)>,
    handler: Handler,
    ctx: usize,
    name: [u8; 16],
    name_len: u8,
}

static TABLE: Mutex<[Option<Entry>; MAX_HANDLERS]> = Mutex::new([None; MAX_HANDLERS]);
static COUNTS: [AtomicU64; MAX_HANDLERS] = [const { AtomicU64::new(0) }; MAX_HANDLERS];
/// Interrupts that reached `dispatch` with no handler registered.
static SPURIOUS: AtomicU64 = AtomicU64::new(0);
/// The [`dispatch`]es running, with a copy of the table: what
/// [`unregister_pci`] waits out before a handler's context may go.
static IN_DISPATCH: AtomicUsize = AtomicUsize::new(0);

/// How a PCI function's interrupt is delivered (chosen per arch).
#[derive(Clone, Copy)]
pub struct PciIrq {
    /// Interrupt number as seen by [`dispatch`] (vector / INTID / PLIC source).
    pub irq: u32,
    pub delivery: Delivery,
}

/// What raises a PCI function's interrupt (each arch has its own).
#[derive(Clone, Copy)]
#[allow(dead_code)]
pub enum Delivery {
    /// The MSI-X table entry the device must use.
    MsiX(u16),
    /// Its one MSI message (a function without MSI-X).
    Msi,
    /// The legacy INTx line: the handler has to read the device's ISR
    /// register to deassert it.
    IntX,
}

/// Register `handler(ctx)` for interrupt `irq`, beside any other handler of
/// it (a shared line). Fails when the table is full or the same handler
/// and context are registered for it already.
pub fn register(irq: u32, name: &str, handler: Handler, ctx: usize) -> bool {
    register_for(irq, None, name, handler, ctx)
}

/// [`register`] for PCI function `pci`'s interrupt, which
/// [`pci_irq_disable`] gives back.
pub fn register_pci(irq: u32, pci: (u8, u8, u8), name: &str, handler: Handler, ctx: usize) -> bool {
    register_for(irq, Some(pci), name, handler, ctx)
}

fn register_for(irq: u32, pci: Option<(u8, u8, u8)>, name: &str, handler: Handler, ctx: usize) -> bool {
    let mut table = TABLE.lock();
    if table.iter().flatten().any(|e| e.irq == irq && e.handler as usize == handler as usize && e.ctx == ctx) {
        return false;
    }
    let Some(slot) = table.iter().position(|e| e.is_none()) else {
        return false;
    };
    let mut n = [0u8; 16];
    let len = name.len().min(16);
    n[..len].copy_from_slice(&name.as_bytes()[..len]);
    COUNTS[slot].store(0, Ordering::Relaxed);
    table[slot] = Some(Entry {
        irq,
        pci,
        handler,
        ctx,
        name: n,
        name_len: len as u8,
    });
    true
}

/// Route platform interrupt `irq` to `handler(ctx)` on CPU 0, through the
/// interrupt controller (`irqchip::root`, or the domain the number is
/// one of): on x86_64 a legacy ISA IRQ (through the I/O APIC), elsewhere
/// the number the device tree or the SPCR gives (a GIC SPI, a PLIC
/// source, a domain's line). The handler is in place before
/// the line is unmasked. False when it cannot be routed: the caller polls
/// its device.
pub fn enable(irq: u32, name: &str, handler: Handler, ctx: usize) -> bool {
    let Some(n) = crate::irqchip::route(irq) else {
        return false;
    };
    if !register(n, name, handler, ctx) {
        return false;
    }
    crate::irqchip::unmask(irq);
    true
}

/// Second-level dispatch from the arch interrupt entry (IRQs masked).
pub fn dispatch(irq: u32) {
    // `try_lock`: a registration racing with an interrupt on another CPU is
    // harmless (the interrupt is simply counted as spurious once); the
    // interrupted CPU can never be the holder because `register` runs with
    // IRQs on only from module init, and we never spin in IRQ context. The
    // handlers run with the table unlocked (copied out).
    IN_DISPATCH.fetch_add(1, Ordering::SeqCst);
    let Some(table) = TABLE.try_lock().map(|t| *t) else {
        IN_DISPATCH.fetch_sub(1, Ordering::SeqCst);
        SPURIOUS.fetch_add(1, Ordering::Relaxed);
        return;
    };
    let mut handled = false;
    for (i, e) in table.iter().enumerate() {
        let Some(e) = e.filter(|e| e.irq == irq) else { continue };
        handled = true;
        COUNTS[i].fetch_add(1, Ordering::Relaxed);
        unsafe { (e.handler)(e.ctx as *mut core::ffi::c_void) };
    }
    IN_DISPATCH.fetch_sub(1, Ordering::SeqCst);
    if !handled {
        SPURIOUS.fetch_add(1, Ordering::Relaxed);
    }
}

/// Route a PCI function's interrupt to CPU 0 and return how it is delivered.
/// x86_64 uses MSI-X (entry 0), or MSI on a function without it, or its
/// INTx line through the I/O APIC where the firmware's `_PRT` routes it;
/// aarch64 and riscv64 use the INTx line the
/// QEMU `virt` machine wires to a GIC SPI / PLIC source.
pub fn pci_irq_setup(bus: u8, slot: u8, func: u8) -> Option<PciIrq> {
    crate::arch::pci_irq_setup(bus, slot, func)
}

/// Take PCI function `pci`'s `handler(ctx)` out (task context): its
/// interrupt number, and whether it was the number's last handler. When
/// this returns the handler runs no more, on any CPU.
fn unregister_pci(pci: (u8, u8, u8), handler: Handler, ctx: usize) -> Option<(u32, bool)> {
    let irq = {
        let mut table = TABLE.lock();
        let slot = table
            .iter()
            .position(|e| e.is_some_and(|e| e.pci == Some(pci) && e.handler as usize == handler as usize && e.ctx == ctx))?;
        let irq = table[slot].map(|e| e.irq)?;
        table[slot] = None;
        irq
    };
    // A dispatch that copied the table before the entry went may still be
    // calling it.
    while IN_DISPATCH.load(Ordering::SeqCst) != 0 {
        core::hint::spin_loop();
    }
    let last = !TABLE.lock().iter().flatten().any(|e| e.irq == irq);
    Some((irq, last))
}

/// Give back what [`pci_irq_setup`] and [`register_pci`] took for PCI
/// function `bus:slot.func`'s `handler(ctx)` (`KernelApi::pci_irq_disable`):
/// the handler's slot; the function's interrupt turned off; and once the
/// interrupt has no handler left, what the arch took for it (x86_64: the
/// vector and the IOMMU's remapping entry, the I/O APIC pin of an INTx
/// line). False when no such handler is registered.
pub fn pci_irq_disable(bus: u8, slot: u8, func: u8, handler: Handler, ctx: usize) -> bool {
    let Some((irq, last)) = unregister_pci((bus, slot, func), handler, ctx) else {
        return false;
    };
    crate::arch::pci_irq_release(bus, slot, func, irq, last);
    true
}

/// The function's INTx off (the command register's disable bit): its
/// interrupt given back on a board whose controller lines are not
/// allocated (aarch64, riscv64), only their handlers.
#[cfg(not(target_arch = "x86_64"))]
pub fn pci_intx_release(bus: u8, slot: u8, func: u8) {
    let cmd = crate::pci::cfg_read32(bus, slot, func, 0x04) & 0xFFFF;
    crate::pci::cfg_write32(bus, slot, func, 0x04, cmd | 1 << 10);
}

/// A PCI function's INTx line where the device tree routes it (the host
/// bridge's `interrupt-map`, `dt::pci_intx`), decoded by the interrupt
/// controller it names and enabled on CPU 0. Legacy INTx: the handler
/// must read the device's ISR to deassert the line.
#[cfg(not(target_arch = "x86_64"))]
pub fn pci_intx_setup(bus: u8, slot: u8, func: u8) -> Option<PciIrq> {
    let pin = (crate::pci::cfg_read32(bus, slot, func, 0x3C) >> 8) & 0xFF;
    if pin == 0 || pin > 4 {
        return None;
    }
    let spec = crate::dt::pci_intx(bus, slot, func, pin as u8)?;
    let irq = crate::irqchip::xlate(&spec)?;
    crate::irqchip::unmask(irq);
    Some(PciIrq { irq, delivery: Delivery::IntX })
}

/// `/proc/interrupts`.
pub fn interrupts_text() -> alloc::vec::Vec<u8> {
    let mut out = alloc::string::String::new();
    let table = TABLE.lock();
    for (i, e) in table.iter().enumerate() {
        let Some(e) = e else { continue };
        let name = core::str::from_utf8(&e.name[..e.name_len as usize]).unwrap_or("?");
        out.push_str(&alloc::format!(
            "{:>4}: {:>12}  {}\n",
            e.irq,
            COUNTS[i].load(Ordering::Relaxed),
            name
        ));
    }
    out.push_str(&alloc::format!(
        "spurious: {}\n",
        SPURIOUS.load(Ordering::Relaxed)
    ));
    // x86_64's device vectors, the one pool handlers take from.
    #[cfg(target_arch = "x86_64")]
    {
        let (used, all) = crate::arch::irq_vectors();
        out.push_str(&alloc::format!("vectors: {used}/{all}\n"));
    }
    out.into_bytes()
}
