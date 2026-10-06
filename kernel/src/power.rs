//! Power-off, reboot and halt (`docs/power.md`). One sequence for every
//! board: the processes are asked to stop (`SIGTERM`), given a moment, then
//! killed; the disk filesystems are unmounted, so what they cache reaches
//! the disk; then the methods that can do the action are tried in order
//! until one takes the machine down. A method that returns has failed.
//!
//! The methods are the arch's own (PSCI, the SBI, the x86 reset paths:
//! `arch::POWER_METHODS`) and the ones modules register before them
//! (`KernelApi::power_register`: the ACPI module's `_S5` power-off and
//! reset register). When none works the machine is left halted, which is
//! what `Halt` asks for directly.

use alloc::string::String;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, Ordering};
use spin::Mutex;

use crate::{arch, console, fs, task, time};

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Off,
    Reboot,
    Halt,
}

impl Action {
    /// From the native `power(action)` argument (`myos_abi::MYOS_POWER_*`).
    pub fn from_raw(raw: usize) -> Option<Action> {
        match u32::try_from(raw).ok()? {
            myos_abi::MYOS_POWER_OFF => Some(Action::Off),
            myos_abi::MYOS_POWER_REBOOT => Some(Action::Reboot),
            myos_abi::MYOS_POWER_HALT => Some(Action::Halt),
            _ => None,
        }
    }

    fn name(self) -> &'static str {
        match self {
            Action::Off => "power off",
            Action::Reboot => "reboot",
            Action::Halt => "halt",
        }
    }
}

/// A way to do an action: it does not return when it works.
pub type Method = unsafe extern "C" fn();

/// The methods modules registered, tried before the arch's in the order
/// they came.
static REGISTERED: Mutex<Vec<(Action, String, Method)>> = Mutex::new(Vec::new());

/// Set by the first `perform`: the system goes down once.
static STARTED: AtomicBool = AtomicBool::new(false);

/// How long the processes have between `SIGTERM` and `SIGKILL`, and to go
/// after it.
const GRACE_NS: u64 = 2_000_000_000;
const KILL_WAIT_NS: u64 = 1_000_000_000;
/// How long a method that returned may still take the machine down (a
/// reset line being pulsed) before the next one is tried.
const SETTLE_NS: u64 = 100_000_000;

/// Add `method` for `action` under `name` (`power: off via NAME`).
pub fn register(action: Action, name: &str, method: Method) -> bool {
    if action == Action::Halt {
        return false;
    }
    REGISTERED.lock().push((action, String::from(name), method));
    true
}

/// Take the system down. Returns, at once, only when it is already going
/// down.
pub fn perform(action: Action) {
    if STARTED.swap(true, Ordering::SeqCst) {
        return;
    }
    console::status_info(&alloc::format!("{}: stopping processes", action.name()));
    stop_processes();
    for prefix in fs::vfs::unmount_all() {
        console::status_warn(&alloc::format!("{}: {prefix} is busy, not unmounted", action.name()));
    }
    let methods: Vec<(String, Method)> = REGISTERED
        .lock()
        .iter()
        .filter(|(a, _, _)| *a == action)
        .map(|(_, name, method)| (name.clone(), *method))
        .chain(
            arch::POWER_METHODS
                .iter()
                .filter(|(a, _, _)| *a == action)
                .map(|(_, name, method)| (String::from(*name), *method)),
        )
        .collect();
    for (name, method) in methods {
        console::status_info(&alloc::format!("{} via {name}", action.name()));
        console::flush();
        unsafe { method() };
        time::spin_ns(SETTLE_NS);
    }
    if action != Action::Halt {
        console::status_fail(&alloc::format!("{}: no method worked", action.name()));
    }
    console::status_info("system halted");
    console::flush();
    arch::irq_off();
    arch::halt();
}

/// `SIGTERM` to every process but the caller's, `SIGKILL` to what is left
/// after the grace period; each time until they are gone or the wait ends.
fn stop_processes() {
    for (sig, wait) in [(crate::signal::SIGTERM, GRACE_NS), (crate::signal::SIGKILL, KILL_WAIT_NS)] {
        let others = others();
        if others.is_empty() {
            return;
        }
        for id in others {
            task::signal_send(id, sig);
        }
        let deadline = time::monotonic_ns() + wait;
        while !self::others().is_empty() && time::monotonic_ns() < deadline {
            // A signal for the caller ends the sleep early: poll again.
            task::sleep_until((time::monotonic_ns() + 10_000_000).min(deadline), false);
        }
    }
}

/// The live processes other than the caller's.
fn others() -> Vec<usize> {
    let me = task::current_pid();
    (0..task::task_slots())
        .filter(|&id| id != me && task::task_pgid(id).is_some() && task::is_live_user(id))
        .collect()
}
