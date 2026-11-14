//! libstd-mlibc-rt.so: the one copy of std in the program (linked into
//! this dylib from the sysroot's rlibs, as rustc's own driver library
//! holds its std), and the state `host` and `plugin` share through it.

use std::sync::Mutex;
use std::sync::atomic::{AtomicU32, Ordering};

static CALLS: AtomicU32 = AtomicU32::new(0);
static NAMES: Mutex<Vec<String>> = Mutex::new(Vec::new());

/// Counts a call, whoever makes it: one counter in the process.
pub fn count() -> u32 {
    CALLS.fetch_add(1, Ordering::SeqCst) + 1
}

pub fn record(name: &str) {
    NAMES.lock().unwrap().push(name.to_string());
}

pub fn recorded() -> Vec<String> {
    NAMES.lock().unwrap().clone()
}

/// What the plugin hands the program: a trait object made on one side of
/// the dlopen and used and dropped on the other.
pub trait Greeter: Send {
    fn greet(&self, who: &str) -> String;
}
