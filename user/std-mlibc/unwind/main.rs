//! Unwinding on the mlibc flavour of `std` (`<arch>-unknown-myos-mlibc`,
//! docs/userspace.md "`std` on mlibc"): a panic unwinds the stack through
//! LLVM's libunwind instead of ending the program.
//!
//! - `catch_unwind` stops a panic and hands its payload over, a `&str` or
//!   a `String` or any value (`panic_any`); the program goes on.
//! - The values of the frames a panic leaves are dropped, innermost
//!   first, a lock held there is released (poisoned), and a panic deep in
//!   a recursion unwinds every frame of it.
//! - `resume_unwind` carries a caught payload on, and a panic in a
//!   destructor that runs during a normal return unwinds too.
//! - A thread that panics ends alone: `join` gives its payload, the other
//!   threads and the heap are as they were.
//! - The panic hook runs before the unwinding, on the panicking thread.
//!
//! Prints `[ OK ] std unwind` or a `[ FAIL ] std unwind: ...` line. The
//! panics' own messages are not printed: the hook counts them.

use std::any::Any;
use std::panic::{self, AssertUnwindSafe, catch_unwind, panic_any, resume_unwind};
use std::process::ExitCode;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;

fn check(ok: bool, what: &str) -> Result<(), String> {
    if ok { Ok(()) } else { Err(what.to_string()) }
}

static HOOKED: AtomicUsize = AtomicUsize::new(0);

/// Records its name when dropped.
struct Trace<'a>(&'a Mutex<Vec<&'static str>>, &'static str);

impl Drop for Trace<'_> {
    fn drop(&mut self) {
        self.0.lock().unwrap().push(self.1);
    }
}

fn message(payload: &(dyn Any + Send)) -> Option<&str> {
    payload.downcast_ref::<&str>().copied().or_else(|| payload.downcast_ref::<String>().map(String::as_str))
}

#[inline(never)]
fn deep(n: u32, log: &Mutex<Vec<&'static str>>) -> u32 {
    let _t = Trace(log, "frame");
    if n == 0 {
        panic!("bottom");
    }
    // Not a tail call: every frame stays on the stack.
    deep(n - 1, log) + 1
}

fn payloads() -> Result<(), String> {
    let r = catch_unwind(|| -> u32 { panic!("a literal") });
    check(r.as_ref().err().and_then(|p| message(&**p)) == Some("a literal"), "a &str payload")?;
    let n = 7;
    let r = catch_unwind(|| -> u32 { panic!("formatted {n}") });
    check(r.as_ref().err().and_then(|p| message(&**p)) == Some("formatted 7"), "a String payload")?;
    let r = catch_unwind(|| -> u32 { panic_any(vec![1u8, 2, 3]) });
    check(r.err().and_then(|p| p.downcast::<Vec<u8>>().ok()).as_deref() == Some(&vec![1u8, 2, 3]), "a value payload")?;
    check(catch_unwind(|| 5).ok() == Some(5), "no panic")
}

fn drops() -> Result<(), String> {
    let log = Mutex::new(Vec::new());
    let r = catch_unwind(AssertUnwindSafe(|| {
        let _outer = Trace(&log, "outer");
        let _inner = Trace(&log, "inner");
        panic!("between");
    }));
    check(r.is_err(), "the panic was not caught")?;
    check(*log.lock().unwrap() == ["inner", "outer"], "the frame's values were not dropped in order")?;

    let log = Mutex::new(Vec::new());
    let r = catch_unwind(AssertUnwindSafe(|| deep(200, &log)));
    check(r.is_err(), "the deep panic was not caught")?;
    check(log.lock().unwrap().len() == 201, "not every frame of the recursion was unwound")?;

    // A lock held where the panic passed is released, and poisoned.
    let lock = Mutex::new(0);
    let r = catch_unwind(AssertUnwindSafe(|| {
        let mut g = lock.lock().unwrap();
        *g = 1;
        panic!("with the lock");
    }));
    check(r.is_err() && lock.is_poisoned(), "the lock was not poisoned")?;
    check(lock.try_lock().is_err_and(|e| matches!(e, std::sync::TryLockError::Poisoned(_))), "the lock was not released")
}

fn resumed() -> Result<(), String> {
    let r = catch_unwind(|| {
        let first = catch_unwind(|| panic!("carried")).unwrap_err();
        resume_unwind(first);
    });
    check(r.as_ref().err().and_then(|p| message(&**p)) == Some("carried"), "resume_unwind's payload")?;

    struct Bomb;
    impl Drop for Bomb {
        fn drop(&mut self) {
            panic!("in a destructor");
        }
    }
    let r = catch_unwind(|| {
        let _b = Bomb;
    });
    check(r.as_ref().err().and_then(|p| message(&**p)) == Some("in a destructor"), "a destructor's panic")
}

fn threads() -> Result<(), String> {
    let shared = Arc::new(Mutex::new(Vec::new()));
    let handles: Vec<_> = (0..4u32)
        .map(|i| {
            let shared = shared.clone();
            thread::spawn(move || {
                let v: Vec<u32> = (0..1000).map(|x| x * i).collect();
                if i % 2 == 1 {
                    panic_any(i);
                }
                shared.lock().unwrap().push(v.iter().sum::<u32>());
                i
            })
        })
        .collect();
    for (i, h) in handles.into_iter().enumerate() {
        match h.join() {
            Ok(v) => check(i % 2 == 0 && v == i as u32, "a thread's value")?,
            Err(p) => check(i % 2 == 1 && p.downcast_ref::<u32>() == Some(&(i as u32)), "a thread's panic payload")?,
        }
    }
    check(shared.lock().map(|g| g.len()).unwrap_or(0) == 2, "the threads that did not panic")
}

fn run() -> Result<(), String> {
    panic::set_hook(Box::new(|_| {
        HOOKED.fetch_add(1, Ordering::SeqCst);
    }));
    payloads()?;
    drops()?;
    resumed()?;
    threads()?;
    // Every panic! above went through the hook (not resume_unwind's, nor
    // panic_any's... which does): 3 payloads, 3 drops, 2 resumed, 2 threads.
    let hooked = HOOKED.load(Ordering::SeqCst);
    check(hooked == 10, &format!("the hook ran {hooked} times, not 10"))
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => {
            println!("[ OK ] std unwind");
            ExitCode::SUCCESS
        }
        Err(e) => {
            println!("[ FAIL ] std unwind: {e}");
            ExitCode::FAILURE
        }
    }
}
