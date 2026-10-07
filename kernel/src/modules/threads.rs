//! Kernel threads for modules (`KernelApi::thread_spawn`): a bus module
//! enumerates devices and runs its class drivers' hooks on one (`docs/usb.md`).
//! The scheduler starts a kernel task at a plain `fn()`, so each of the
//! [`MAX`] slots has its own trampoline that fetches the module's entry and
//! argument from the table.

use core::ffi::c_void;
use spin::Mutex;

pub use myos_abi::MYOS_MAX_MODULE_THREADS as MAX;

type Entry = unsafe extern "C" fn(*mut c_void);

static THREADS: Mutex<[Option<(Entry, usize)>; MAX]> = Mutex::new([None; MAX]);

/// Start `entry(ctx)` on a new kernel task named `name` in `/proc`: false
/// when every slot is taken.
pub fn spawn(name: &str, entry: Entry, ctx: *mut c_void) -> bool {
    const TRAMPOLINES: [fn(); MAX] = [
        || run(0),
        || run(1),
        || run(2),
        || run(3),
        || run(4),
        || run(5),
        || run(6),
        || run(7),
    ];
    let slot = {
        let mut threads = THREADS.lock();
        let Some(slot) = threads.iter().position(|t| t.is_none()) else {
            return false;
        };
        threads[slot] = Some((entry, ctx as usize));
        slot
    };
    crate::task::spawn_named(name.as_bytes(), TRAMPOLINES[slot]);
    true
}

fn run(slot: usize) {
    let Some((entry, ctx)) = THREADS.lock().get(slot).copied().flatten() else {
        return;
    };
    unsafe { entry(ctx as *mut c_void) };
    // The entry returned: the task ends (`task::spawn`'s trampoline calls
    // `die`); the slot stays taken, since the module may be unloaded only
    // when it registered nothing, and a thread counts.
}
