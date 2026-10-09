//! Device interrupt registry: one handler per interrupt number, shared by
//! the three architectures' second-level dispatch.
//!
//! * x86_64: MSI-X messages and I/O APIC pins (the legacy ISA interrupts)
//!   land on LAPIC vectors `DEVICE_VECTOR_BASE..`; the IDT stubs call
//!   [`dispatch`] with the vector.
//! * aarch64: PCI INTx lines and the board's devices are GIC SPIs;
//!   `aarch64_irq_handler` passes any non-timer, non-SGI INTID here.
//! * riscv64: PCI INTx lines and the board's devices are PLIC sources; the
//!   supervisor external interrupt claims from the PLIC and passes the
//!   source id here.
//!
//! Handlers run in interrupt context on the CPU that took the interrupt
//! (CPU 0: every interrupt is routed to the BSP). They ack the device
//! (virtio ISR read) and typically `task::wake*` a sleeper.
//!
//! Modules get this through `KernelApi::pci_irq_enable`, which also picks
//! the delivery mechanism per arch (`pci_irq_setup`), and
//! `KernelApi::irq_enable` for a board device's own interrupt ([`enable`]).

use core::sync::atomic::{AtomicU64, Ordering};
use spin::Mutex;

pub type Handler = unsafe extern "C" fn(ctx: *mut core::ffi::c_void);

const MAX_HANDLERS: usize = 16;

#[derive(Clone, Copy)]
struct Entry {
    irq: u32,
    handler: Handler,
    ctx: usize,
    name: [u8; 16],
    name_len: u8,
}

static TABLE: Mutex<[Option<Entry>; MAX_HANDLERS]> = Mutex::new([None; MAX_HANDLERS]);
static COUNTS: [AtomicU64; MAX_HANDLERS] = [const { AtomicU64::new(0) }; MAX_HANDLERS];
/// Interrupts that reached `dispatch` with no handler registered.
static SPURIOUS: AtomicU64 = AtomicU64::new(0);

/// How a PCI function's interrupt is delivered (chosen per arch).
#[derive(Clone, Copy)]
pub struct PciIrq {
    /// Interrupt number as seen by [`dispatch`] (vector / INTID / PLIC source).
    pub irq: u32,
    /// MSI-X table entry the device must use (`None` = legacy INTx: the
    /// handler has to read the device's ISR register to deassert the line).
    pub msix_entry: Option<u16>,
}

/// Register `handler(ctx)` for interrupt `irq`. Fails when the table is full
/// or the irq already has a handler.
pub fn register(irq: u32, name: &str, handler: Handler, ctx: usize) -> bool {
    let mut table = TABLE.lock();
    if table.iter().flatten().any(|e| e.irq == irq) {
        return false;
    }
    let Some(slot) = table.iter().position(|e| e.is_none()) else {
        return false;
    };
    let mut n = [0u8; 16];
    let len = name.len().min(16);
    n[..len].copy_from_slice(&name.as_bytes()[..len]);
    table[slot] = Some(Entry {
        irq,
        handler,
        ctx,
        name: n,
        name_len: len as u8,
    });
    true
}

/// Route platform interrupt `irq` to `handler(ctx)` on CPU 0: on x86_64 a
/// legacy ISA IRQ (through the I/O APIC), elsewhere a GIC SPI or a PLIC
/// source (the number the device tree or the SPCR gives). The handler is in
/// place before the line is unmasked. False when it cannot be routed: the
/// caller polls its device.
pub fn enable(irq: u32, name: &str, handler: Handler, ctx: usize) -> bool {
    let Some(n) = crate::arch::irq_route(irq) else {
        return false;
    };
    if !register(n, name, handler, ctx) {
        return false;
    }
    crate::arch::irq_unmask(irq);
    true
}

/// Second-level dispatch from the arch interrupt entry (IRQs masked).
pub fn dispatch(irq: u32) {
    // `try_lock`: a registration racing with an interrupt on another CPU is
    // harmless (the interrupt is simply counted as spurious once); the
    // interrupted CPU can never be the holder because `register` runs with
    // IRQs on only from module init, and we never spin in IRQ context.
    let entry = TABLE
        .try_lock()
        .and_then(|t| t.iter().enumerate().find_map(|(i, e)| e.filter(|e| e.irq == irq).map(|e| (i, e))));
    match entry {
        Some((i, e)) => {
            COUNTS[i].fetch_add(1, Ordering::Relaxed);
            unsafe { (e.handler)(e.ctx as *mut core::ffi::c_void) };
        }
        None => {
            SPURIOUS.fetch_add(1, Ordering::Relaxed);
        }
    }
}

/// Route a PCI function's interrupt to CPU 0 and return how it is delivered.
/// x86_64 uses MSI-X (entry 0); aarch64 and riscv64 use the INTx line the
/// QEMU `virt` machine wires to a GIC SPI / PLIC source.
pub fn pci_irq_setup(bus: u8, slot: u8, func: u8) -> Option<PciIrq> {
    crate::arch::pci_irq_setup(bus, slot, func)
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
    out.into_bytes()
}
