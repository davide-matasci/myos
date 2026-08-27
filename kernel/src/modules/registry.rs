//! Loaded-module table. Enough to look up name / base / init / exit.

use alloc::vec::Vec;
use myos_abi::{ModuleExit, ModuleInit};
use spin::Mutex;

#[derive(Clone, Copy)]
pub struct LoadedModule {
    pub name: &'static str,
    pub base: usize,
    pub size: usize,
    pub init: Option<ModuleInit>,
    pub exit: Option<ModuleExit>,
}

static MODULES: Mutex<Vec<LoadedModule>> = Mutex::new(Vec::new());

pub fn register(module: LoadedModule) {
    MODULES.lock().push(module);
}

pub fn count() -> usize {
    MODULES.lock().len()
}

pub fn by_name(name: &str) -> Option<LoadedModule> {
    MODULES.lock().iter().copied().find(|m| m.name == name)
}
