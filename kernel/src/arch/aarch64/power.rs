//! PSCI power-off and reset (`crate::power`), called through the conduit
//! the platform names.

use crate::power::{Action, Method};

/// PSCI 0.2 `SYSTEM_OFF` and `SYSTEM_RESET`.
pub const SYSTEM_OFF: u64 = 0x8400_0008;
const SYSTEM_RESET: u64 = 0x8400_0009;

pub const POWER_METHODS: &[(Action, &str, Method)] =
    &[(Action::Off, "psci", psci_off), (Action::Reboot, "psci", psci_reset)];

unsafe extern "C" fn psci_off() {
    psci(SYSTEM_OFF);
}

unsafe extern "C" fn psci_reset() {
    psci(SYSTEM_RESET);
}

/// Call PSCI function `func` through the conduit the device tree's `/psci`
/// or the FADT names; without one (before the platform is described, a
/// board that does not say), HVC at EL1 (a hypervisor's PSCI) and SMC at
/// EL2. Returns only when the call failed.
pub fn psci(func: u64) {
    use crate::platform::PsciConduit;
    let hvc = match crate::platform::get().psci.map(|c| c.value) {
        Some(PsciConduit::Hvc) => true,
        Some(PsciConduit::Smc) => false,
        None => super::current_el() < 2,
    };
    unsafe {
        if hvc {
            core::arch::asm!("hvc #0", inout("x0") func => _, options(nostack), clobber_abi("C"));
        } else {
            core::arch::asm!("smc #0", inout("x0") func => _, options(nostack), clobber_abi("C"));
        }
    }
}
