//! Message-signalled interrupts: a PCI function raises its interrupt by
//! writing a value (the message's data) to an address (the interrupt
//! controller's doorbell), through its MSI-X table or its MSI capability.
//! The configuration space side is the same on every arch ([`msix_enable`],
//! [`msi_enable`], [`disable`]); which address and value raise which
//! interrupt is the controller's: x86_64's local APIC (with the IOMMU's
//! remapping, `arch/x86/interrupts.rs`), elsewhere an MSI controller
//! ([`crate::irqchip::MsiChip`]: the GICv3's ITS, the GICv2m frame, the
//! RISC-V IMSIC), which [`pci_setup`] asks for an interrupt.

/// What a function writes to raise an interrupt.
#[derive(Clone, Copy)]
pub struct Message {
    pub address: u64,
    pub data: u32,
}

/// The MSI capability's id and the MSI-X one's.
const CAP_MSI: u32 = 0x05;
const CAP_MSIX: u32 = 0x11;
/// The command register's INTx disable bit.
const CMD_INTX_OFF: u32 = 1 << 10;

/// The offset of the PCI function's capability `id` in configuration
/// space, if it has one.
pub fn cap(bus: u8, slot: u8, func: u8, id: u32) -> Option<u8> {
    let cfg = |off: u8| crate::pci::cfg_read32(bus, slot, func, off);
    let status = (cfg(4) >> 16) as u16;
    if status & 0x10 == 0 {
        return None;
    }
    let mut cap = (cfg(0x34) & 0xFC) as u8;
    let mut hops = 0;
    while cap != 0 && hops < 48 {
        hops += 1;
        let w0 = cfg(cap);
        if w0 & 0xFF == id {
            return Some(cap);
        }
        cap = ((w0 >> 8) & 0xFC) as u8;
    }
    None
}

/// Whether the function has MSI-X.
pub fn has_msix(bus: u8, slot: u8, func: u8) -> bool {
    cap(bus, slot, func, CAP_MSIX).is_some()
}

/// Whether the function has MSI.
pub fn has_msi(bus: u8, slot: u8, func: u8) -> bool {
    cap(bus, slot, func, CAP_MSI).is_some()
}

/// Program MSI-X table entry 0 of the function with `msg` and turn MSI-X
/// on (the function mask off). The caller still has to point the device's
/// queue at entry 0 (virtio: `queue_msix_vector`). False without MSI-X or
/// its table.
pub fn msix_enable(bus: u8, slot: u8, func: u8, msg: Message) -> bool {
    let cfg = |off: u8| crate::pci::cfg_read32(bus, slot, func, off);
    let Some(cap) = cap(bus, slot, func, CAP_MSIX) else {
        return false;
    };
    let table = cfg(cap.wrapping_add(4));
    let bir = (table & 0x7) as u8;
    let offset = (table & !0x7) as u64;
    let Some((va, size)) = crate::pci::bar_map(bus, slot, func, bir) else {
        return false;
    };
    if offset.saturating_add(16) > size {
        return false;
    }
    let entry = va + offset as usize;
    unsafe {
        // Mask the entry while programming it (vector control bit 0).
        core::ptr::write_volatile((entry + 12) as *mut u32, 1);
        core::ptr::write_volatile(entry as *mut u32, msg.address as u32);
        core::ptr::write_volatile((entry + 4) as *mut u32, (msg.address >> 32) as u32);
        core::ptr::write_volatile((entry + 8) as *mut u32, msg.data);
        core::ptr::write_volatile((entry + 12) as *mut u32, 0);
    }
    // Message control (cap+2): bit 15 MSI-X enable, bit 14 function mask.
    let w0 = cfg(cap);
    let ctrl = ((w0 >> 16) as u16 | 0x8000) & !0x4000;
    crate::pci::cfg_write32(bus, slot, func, cap, (w0 & 0xFFFF) | (u32::from(ctrl) << 16));
    true
}

/// Program the function's one MSI message with `msg` and turn MSI on.
/// False without MSI, or with a 32-bit capability and an address above
/// 4 GiB.
pub fn msi_enable(bus: u8, slot: u8, func: u8, msg: Message) -> bool {
    let cfg = |off: u8| crate::pci::cfg_read32(bus, slot, func, off);
    let write = |off: u8, v: u32| crate::pci::cfg_write32(bus, slot, func, off, v);
    let Some(cap) = cap(bus, slot, func, CAP_MSI) else {
        return false;
    };
    // Message control (cap+2): bit 0 enable, bits 4..6 the vectors enabled
    // (0: one), bit 7 a 64-bit address (the data then at cap+12, not +8).
    let w0 = cfg(cap);
    let ctrl = (w0 >> 16) as u16;
    let wide = ctrl & 0x80 != 0;
    if !wide && msg.address >> 32 != 0 {
        return false;
    }
    write(cap.wrapping_add(4), msg.address as u32);
    let data = if wide {
        write(cap.wrapping_add(8), (msg.address >> 32) as u32);
        cap.wrapping_add(12)
    } else {
        cap.wrapping_add(8)
    };
    write(data, (cfg(data) & 0xFFFF_0000) | (msg.data & 0xFFFF));
    let ctrl = (ctrl & !0x70) | 1;
    write(cap, (w0 & 0xFFFF) | (u32::from(ctrl) << 16));
    true
}

/// Turn the function's MSI-X or MSI off, if one is on, and its INTx too
/// (without its messages it would fall back to it): whether one was on.
pub fn disable(bus: u8, slot: u8, func: u8) -> bool {
    let cfg = |off: u8| crate::pci::cfg_read32(bus, slot, func, off);
    let write = |off: u8, v: u32| crate::pci::cfg_write32(bus, slot, func, off, v);
    let on = if let Some(cap) = cap(bus, slot, func, CAP_MSIX).filter(|cap| cfg(*cap) & 1 << 31 != 0) {
        // MSI-X off (message control bit 15), its function mask on (14).
        write(cap, (cfg(cap) & !(1 << 31)) | 1 << 30);
        true
    } else if let Some(cap) = cap(bus, slot, func, CAP_MSI).filter(|cap| cfg(*cap) & 1 << 16 != 0) {
        write(cap, cfg(cap) & !(1 << 16));
        true
    } else {
        false
    };
    if on {
        write(4, (cfg(4) & 0xFFFF) | CMD_INTX_OFF);
    }
    on
}

/// A PCI function's interrupt through the board's MSI controller: its
/// MSI-X (entry 0), else its MSI. `None` without a controller, without
/// either capability, or with the controller's interrupts all taken: the
/// caller takes the function's INTx.
#[cfg(not(target_arch = "x86_64"))]
pub fn pci_setup(bus: u8, slot: u8, func: u8) -> Option<crate::irq::PciIrq> {
    use crate::irq::{Delivery, PciIrq};
    let chip = crate::irqchip::msi()?;
    let msix = has_msix(bus, slot, func);
    let msi = has_msi(bus, slot, func);
    if !msix && !msi {
        return None;
    }
    let rid = u16::from(bus) << 8 | u16::from(slot) << 3 | u16::from(func);
    let (irq, msg) = chip.alloc(rid)?;
    // A device behind the IOMMU in a domain of its own reaches the
    // controller through the unit, which may give it a message of its own.
    let msg = crate::iommu::msi_message(bus, slot, func, irq, msg);
    if msix && msix_enable(bus, slot, func, msg) {
        return Some(PciIrq { irq, delivery: Delivery::MsiX(0) });
    }
    if msi && msi_enable(bus, slot, func, msg) {
        return Some(PciIrq { irq, delivery: Delivery::Msi });
    }
    crate::iommu::msi_release(bus, slot, func);
    chip.free(irq);
    None
}

/// Give back what [`pci_setup`] took for the function's interrupt `irq`:
/// its MSI-X or MSI off, what the IOMMU made for it, the controller's
/// interrupt freed. Whether it had one on.
#[cfg(not(target_arch = "x86_64"))]
pub fn pci_release(bus: u8, slot: u8, func: u8, irq: u32) -> bool {
    if !disable(bus, slot, func) {
        return false;
    }
    crate::iommu::msi_release(bus, slot, func);
    if let Some(chip) = crate::irqchip::msi() {
        chip.free(irq);
    }
    true
}
