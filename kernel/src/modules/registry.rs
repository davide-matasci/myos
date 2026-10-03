//! Loaded-module table: the modules whose `init` succeeded, with their
//! image (for `rmmod`), their optional entry points and a count of what they
//! registered through the `KernelApi` (block and char devices, filesystems,
//! mounts, `/proc` nodes, interrupts, the console, a personality). A module
//! that still provides something cannot be unloaded: the kernel has no
//! unregister paths yet, so the count is the guard.

use alloc::vec::Vec;
use myos_abi::{ModuleExit, ModuleRescan};
use spin::Mutex;

#[derive(Clone, Copy)]
pub struct LoadedModule {
    pub name: &'static str,
    /// The image's address and size (`elf::free_image`).
    pub base: usize,
    pub size: usize,
    pub exit: Option<ModuleExit>,
    pub rescan: Option<ModuleRescan>,
    /// Registrations made through the `KernelApi`, by this module's
    /// `module_init` or `module_rescan`.
    pub registrations: u32,
}

static MODULES: Mutex<Vec<LoadedModule>> = Mutex::new(Vec::new());

/// The module whose `module_init` or `module_rescan` is running on behalf
/// of the kernel right now: its registrations are counted against it.
static CURRENT: Mutex<Option<&'static str>> = Mutex::new(None);

pub fn register(module: LoadedModule) {
    MODULES.lock().push(module);
}

pub fn by_name(name: &str) -> Option<LoadedModule> {
    MODULES.lock().iter().copied().find(|m| m.name == name)
}

/// Every loaded module, in load order.
pub fn all() -> Vec<LoadedModule> {
    MODULES.lock().clone()
}

/// Take `name` out of the table when it registered nothing; otherwise the
/// number of its registrations.
pub fn remove_if_free(name: &str) -> Result<Option<LoadedModule>, u32> {
    let mut modules = MODULES.lock();
    let Some(i) = modules.iter().position(|m| m.name == name) else {
        return Ok(None);
    };
    if modules[i].registrations > 0 {
        return Err(modules[i].registrations);
    }
    Ok(Some(modules.remove(i)))
}

/// Run `f` as module `name`: what it registers meanwhile counts for it.
pub fn as_module<T>(name: &'static str, f: impl FnOnce() -> T) -> T {
    *CURRENT.lock() = Some(name);
    let out = f();
    *CURRENT.lock() = None;
    out
}

/// A successful registration through the `KernelApi`, for the module whose
/// call is running (none when the kernel itself registers).
pub fn note_registration() {
    let Some(name) = *CURRENT.lock() else {
        return;
    };
    if let Some(m) = MODULES.lock().iter_mut().find(|m| m.name == name) {
        m.registrations += 1;
    }
}
