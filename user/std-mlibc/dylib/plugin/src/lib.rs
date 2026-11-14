//! libplugin.so: what `host` loads with dlopen. Its std is `rt`'s, the
//! program's own: what it allocates the program frees, and its panics
//! unwind into the program's frames.

use std::cell::Cell;
use std::thread;

thread_local! {
    static CALLS: Cell<u32> = const { Cell::new(0) };
}

struct Hello(String);

impl rt::Greeter for Hello {
    fn greet(&self, who: &str) -> String {
        format!("{}, {who}", self.0)
    }
}

impl Drop for Hello {
    fn drop(&mut self) {
        rt::record("dropped");
    }
}

/// A boxed trait object for the program to use and drop.
#[unsafe(no_mangle)]
pub extern "C-unwind" fn plugin_greeter() -> *mut Box<dyn rt::Greeter> {
    rt::count();
    Box::into_raw(Box::new(Box::new(Hello("hello".to_string()))))
}

/// The sum of `0..n` and this thread's count of calls; `n == 0` panics,
/// with a value on the frame to drop on the way out.
#[unsafe(no_mangle)]
pub extern "C-unwind" fn plugin_sum(n: u32) -> u32 {
    struct Leaving;
    impl Drop for Leaving {
        fn drop(&mut self) {
            rt::record("unwound");
        }
    }
    let _l = Leaving;
    rt::count();
    let calls = CALLS.with(|c| {
        c.set(c.get() + 1);
        c.get()
    });
    if n == 0 {
        panic!("in the plugin");
    }
    (0..n).collect::<Vec<u32>>().iter().sum::<u32>() + calls
}

/// A thread of the plugin's own, joined here.
#[unsafe(no_mangle)]
pub extern "C-unwind" fn plugin_thread() -> u32 {
    thread::spawn(|| plugin_sum(4)).join().unwrap_or(0)
}
