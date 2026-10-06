//! The PC's reset paths (`crate::power`), after the ACPI module's: the
//! reset control register, the keyboard controller, a triple fault. A PC
//! powers off only through ACPI (`modules/acpi`).

use crate::power::{Action, Method};

pub const POWER_METHODS: &[(Action, &str, Method)] = &[
    (Action::Reboot, "reset control register", reset_control),
    (Action::Reboot, "keyboard controller", keyboard_controller),
    (Action::Reboot, "triple fault", triple_fault),
];

fn outb(port: u16, value: u8) {
    unsafe {
        core::arch::asm!("out dx, al", in("dx") port, in("al") value, options(nomem, nostack, preserves_flags));
    }
}

fn inb(port: u16) -> u8 {
    let value: u8;
    unsafe {
        core::arch::asm!("in al, dx", in("dx") port, out("al") value, options(nomem, nostack, preserves_flags));
    }
    value
}

/// The chipset's reset control register (0xCF9: PIIX, ICH): a system reset,
/// then a full one (power cycled) on the 0 → 1 edge of bit 2.
unsafe extern "C" fn reset_control() {
    outb(0xCF9, 0x02);
    crate::time::spin_ns(50_000);
    outb(0xCF9, 0x0E);
}

/// The 8042's pulse of the reset line, once its input buffer is empty.
unsafe extern "C" fn keyboard_controller() {
    for _ in 0..100_000 {
        if inb(0x64) & 2 == 0 {
            break;
        }
    }
    outb(0x64, 0xFE);
}

/// An exception with an empty IDT: the CPU shuts down, and the board
/// resets it.
unsafe extern "C" fn triple_fault() {
    #[repr(C, packed)]
    struct Idtr {
        limit: u16,
        base: u64,
    }
    let empty = Idtr { limit: 0, base: 0 };
    unsafe {
        core::arch::asm!("lidt [{}]", "int3", in(reg) &empty, options(nostack));
    }
}
