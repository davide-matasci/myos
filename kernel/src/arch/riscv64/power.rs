//! SBI power-off and reset (`crate::power`).

use crate::power::{Action, Method};

/// The System Reset extension (`SRST`) and its reset types.
const SRST: u64 = 0x5352_5354;
pub const SHUTDOWN: u64 = 0;
const COLD_REBOOT: u64 = 1;
/// The legacy (SBI 0.1) shutdown, for firmware without `SRST`.
const LEGACY_SHUTDOWN: u64 = 8;

pub const POWER_METHODS: &[(Action, &str, Method)] = &[
    (Action::Off, "sbi", sbi_off),
    (Action::Off, "sbi 0.1", sbi_legacy_off),
    (Action::Reboot, "sbi", sbi_reboot),
];

unsafe extern "C" fn sbi_off() {
    system_reset(SHUTDOWN);
}

unsafe extern "C" fn sbi_reboot() {
    system_reset(COLD_REBOOT);
}

unsafe extern "C" fn sbi_legacy_off() {
    unsafe {
        core::arch::asm!("ecall", in("a7") LEGACY_SHUTDOWN, lateout("a0") _, options(nostack));
    }
}

/// `sbi_system_reset(kind, no reason)`: returns only when it failed.
pub fn system_reset(kind: u64) {
    unsafe {
        core::arch::asm!(
            "ecall",
            in("a7") SRST,
            in("a6") 0u64,
            inout("a0") kind => _,
            inout("a1") 0u64 => _,
            options(nostack),
        );
    }
}
