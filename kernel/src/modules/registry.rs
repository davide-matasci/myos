//! Loaded-module table: names of the modules whose `init` succeeded.

use alloc::vec::Vec;
use spin::Mutex;

#[derive(Clone, Copy)]
pub struct LoadedModule {
    pub name: &'static str,
}

static MODULES: Mutex<Vec<LoadedModule>> = Mutex::new(Vec::new());

pub fn register(module: LoadedModule) {
    MODULES.lock().push(module);
}

pub fn by_name(name: &str) -> Option<LoadedModule> {
    MODULES.lock().iter().copied().find(|m| m.name == name)
}
