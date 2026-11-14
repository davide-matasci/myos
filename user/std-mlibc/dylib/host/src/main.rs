//! Rust shared objects on the mlibc flavour of `std` (docs/userspace.md
//! "`std` on mlibc"): what a compiler does with its plugins.
//!
//! The program is linked against librt.so, a Rust `dylib` that holds std
//! (found beside it, `$ORIGIN`), and loads libplugin.so, another Rust
//! `dylib` over the same librt.so, with `dlopen` (its path is argv[1]).
//!
//! - One std in the process: the plugin's calls into `rt` and the
//!   program's count on one counter, and a boxed trait object the plugin
//!   makes is used and dropped by the program (one heap, one vtable).
//! - A panic in the plugin unwinds out of the shared object into the
//!   program, which catches it with its payload; the plugin's frame was
//!   unwound (its value dropped), and the plugin works afterwards.
//! - Thread-locals of the plugin are each thread's own, on the program's
//!   threads and on one the plugin starts.
//!
//! Prints `[ OK ] std dylib` or a `[ FAIL ] std dylib: ...` line.

use std::ffi::{CStr, CString, c_char, c_int, c_void};
use std::panic::catch_unwind;
use std::process::ExitCode;
use std::thread;

unsafe extern "C" {
    fn dlopen(path: *const c_char, flags: c_int) -> *mut c_void;
    fn dlsym(handle: *mut c_void, name: *const c_char) -> *mut c_void;
    fn dlerror() -> *const c_char;
}

const RTLD_NOW: c_int = 2;

fn check(ok: bool, what: &str) -> Result<(), String> {
    if ok { Ok(()) } else { Err(what.to_string()) }
}

fn symbol(handle: *mut c_void, name: &CStr) -> Result<*mut c_void, String> {
    let p = unsafe { dlsym(handle, name.as_ptr()) };
    check(!p.is_null(), &format!("dlsym {name:?}")).map(|()| p)
}

fn run(plugin: &str) -> Result<(), String> {
    let path = CString::new(plugin).map_err(|_| "the path")?;
    let handle = unsafe { dlopen(path.as_ptr(), RTLD_NOW) };
    if handle.is_null() {
        let why = unsafe { CStr::from_ptr(dlerror()) }.to_string_lossy().into_owned();
        return Err(format!("dlopen: {why}"));
    }
    let greeter: extern "C-unwind" fn() -> *mut Box<dyn rt::Greeter> =
        unsafe { std::mem::transmute(symbol(handle, c"plugin_greeter")?) };
    let sum: extern "C-unwind" fn(u32) -> u32 = unsafe { std::mem::transmute(symbol(handle, c"plugin_sum")?) };
    let on_thread: extern "C-unwind" fn() -> u32 =
        unsafe { std::mem::transmute(symbol(handle, c"plugin_thread")?) };

    // One std, one heap: the plugin's object, used and dropped here.
    let g = unsafe { Box::from_raw(greeter()) };
    check(g.greet("myos") == "hello, myos", "the plugin's trait object")?;
    drop(g);
    check(rt::recorded() == ["dropped"], "the plugin's object was not dropped by the program")?;

    // 0 + 1 + 2 + 3 + 4 = 10, and this thread's first call.
    check(sum(5) == 11, "a call into the plugin")?;

    // A panic there unwinds into this frame.
    let caught = catch_unwind(|| sum(0));
    let payload = caught.err().ok_or("the plugin's panic did not arrive")?;
    check(payload.downcast_ref::<&str>() == Some(&"in the plugin"), "the panic's payload")?;
    check(rt::recorded() == ["dropped", "unwound", "unwound"], "the plugin's frame was not unwound")?;
    // The plugin goes on working: this thread's third call.
    check(sum(1) == 3, "the plugin after its panic")?;

    // Its thread-local is each thread's own.
    let on_ours = thread::spawn(move || sum(4)).join().map_err(|_| "a thread of the program panicked")?;
    check(on_ours == 7, "the plugin's thread-local on the program's thread")?;
    check(on_thread() == 7, "the plugin's thread-local on its own thread")?;
    let lost = thread::spawn(move || sum(0)).join();
    check(lost.is_err(), "a panicking thread in the plugin")?;

    // greeter, sum x6: every call counted in the one `rt`, and this one.
    let calls = rt::count();
    check(calls == 8, &format!("rt counted {calls} calls, not 8"))
}

fn main() -> ExitCode {
    // The panics are the test's own: not a line each on the console.
    std::panic::set_hook(Box::new(|_| {}));
    let plugin = std::env::args().nth(1);
    match plugin.as_deref().ok_or("usage: dylib PLUGIN".to_string()).and_then(run) {
        Ok(()) => {
            println!("[ OK ] std dylib");
            ExitCode::SUCCESS
        }
        Err(e) => {
            println!("[ FAIL ] std dylib: {e}");
            ExitCode::FAILURE
        }
    }
}
