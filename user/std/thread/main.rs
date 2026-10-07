//! `std::thread` on the myos std: threads run in parallel, share memory
//! through `Arc` and the locks, keep their own thread-locals (destroyed
//! when they end), and their stacks and task slots are freed whether they
//! are joined or detached.
#![no_main]

use std::cell::Cell;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Barrier, Condvar, Mutex, RwLock};
use std::thread;
use std::time::Duration;

fn fail(what: &str) -> ! {
    println!("[ FAIL ] std thread: {what}");
    std::process::exit(1);
}

fn check(ok: bool, what: &str) {
    if !ok {
        fail(what);
    }
}

/// Threads incrementing one counter under a mutex and one atomic.
fn counters() {
    const THREADS: usize = 4;
    const ROUNDS: usize = 2000;
    let locked = Arc::new(Mutex::new(0usize));
    let atomic = Arc::new(AtomicUsize::new(0));
    let handles: Vec<_> = (0..THREADS)
        .map(|i| {
            let (locked, atomic) = (locked.clone(), atomic.clone());
            thread::spawn(move || {
                for _ in 0..ROUNDS {
                    *locked.lock().unwrap() += 1;
                    atomic.fetch_add(1, Ordering::Relaxed);
                }
                i * 10
            })
        })
        .collect();
    let results: Vec<usize> = handles.into_iter().map(|h| h.join().unwrap()).collect();
    check(results == [0, 10, 20, 30], "join results");
    check(*locked.lock().unwrap() == THREADS * ROUNDS, "mutex counter");
    check(atomic.load(Ordering::Relaxed) == THREADS * ROUNDS, "atomic counter");
}

static DROPPED: AtomicUsize = AtomicUsize::new(0);

struct Noted(usize);

impl Drop for Noted {
    fn drop(&mut self) {
        DROPPED.fetch_add(self.0, Ordering::SeqCst);
    }
}

thread_local! {
    static MINE: Cell<usize> = const { Cell::new(0) };
    static NOTED: Noted = Noted(0);
}

/// Each thread its own thread-locals; a thread's are dropped when it ends.
fn thread_locals() {
    MINE.with(|m| m.set(7));
    let handles: Vec<_> = (1..=3)
        .map(|i| {
            thread::spawn(move || {
                check(MINE.with(|m| m.get()) == 0, "a new thread's thread-local");
                MINE.with(|m| m.set(i));
                NOTED.with(|_| {});
                thread::sleep(Duration::from_millis(10));
                MINE.with(|m| m.get())
            })
        })
        .collect();
    for (i, h) in (1..=3).zip(handles) {
        check(h.join().unwrap() == i, "a thread's thread-local");
    }
    check(MINE.with(|m| m.get()) == 7, "the main thread's thread-local");
    // `Noted(0)` adds nothing: count the drops with a value of their own.
    thread::spawn(|| {
        thread_local!(static ONE: Noted = Noted(1));
        ONE.with(|_| {});
    })
    .join()
    .unwrap();
    check(DROPPED.load(Ordering::SeqCst) == 1, "thread-local destructors");
}

/// Channels, a condition variable, a barrier and a read-write lock.
fn sync() {
    let (tx, rx) = mpsc::channel();
    for i in 0..3 {
        let tx = tx.clone();
        thread::spawn(move || tx.send(i).unwrap());
    }
    drop(tx);
    let mut got: Vec<i32> = rx.iter().collect();
    got.sort();
    check(got == [0, 1, 2], "channel");

    let pair = Arc::new((Mutex::new(false), Condvar::new()));
    let p = pair.clone();
    let h = thread::spawn(move || {
        thread::sleep(Duration::from_millis(20));
        *p.0.lock().unwrap() = true;
        p.1.notify_one();
    });
    let mut ready = pair.0.lock().unwrap();
    while !*ready {
        ready = pair.1.wait(ready).unwrap();
    }
    drop(ready);
    h.join().unwrap();
    let (lock, cv) = &*pair;
    let (_guard, timeout) = cv.wait_timeout(lock.lock().unwrap(), Duration::from_millis(20)).unwrap();
    check(timeout.timed_out(), "condvar timeout");

    let barrier = Arc::new(Barrier::new(4));
    let passed = Arc::new(AtomicUsize::new(0));
    let handles: Vec<_> = (0..3)
        .map(|_| {
            let (b, p) = (barrier.clone(), passed.clone());
            thread::spawn(move || {
                b.wait();
                p.fetch_add(1, Ordering::SeqCst);
            })
        })
        .collect();
    thread::sleep(Duration::from_millis(20));
    check(passed.load(Ordering::SeqCst) == 0, "barrier held");
    barrier.wait();
    handles.into_iter().for_each(|h| h.join().unwrap());
    check(passed.load(Ordering::SeqCst) == 3, "barrier passed");

    let rw = RwLock::new(vec![1, 2, 3]);
    thread::scope(|s| {
        for _ in 0..3 {
            s.spawn(|| check(rw.read().unwrap().len() == 3, "rwlock read"));
        }
    });
    rw.write().unwrap().push(4);
    check(rw.read().unwrap().len() == 4, "rwlock write");
}

/// Park and unpark, scoped threads borrowing the stack.
fn park_and_scope() {
    let flag = Arc::new(AtomicUsize::new(0));
    let f = flag.clone();
    let main = thread::current();
    let h = thread::spawn(move || {
        f.store(1, Ordering::SeqCst);
        main.unpark();
    });
    while flag.load(Ordering::SeqCst) == 0 {
        thread::park();
    }
    h.join().unwrap();

    let mut data = [1u64, 2, 3, 4];
    thread::scope(|s| {
        for x in data.iter_mut() {
            s.spawn(move || *x *= 10);
        }
    });
    check(data == [10, 20, 30, 40], "scoped threads");
    check(thread::current().id() != thread::spawn(|| thread::current().id()).join().unwrap(), "thread ids");
    let named = thread::Builder::new().name("worker".into()).spawn(|| thread::current().name().map(String::from));
    check(named.unwrap().join().unwrap().as_deref() == Some("worker"), "thread name");
}

/// More threads than the kernel has task slots, one after the other,
/// joined and detached: each frees its slot and its mapping.
fn many() {
    for i in 0..80 {
        let h = thread::Builder::new().stack_size(64 * 1024).spawn(move || i + 1).unwrap();
        if h.join().unwrap() != i + 1 {
            fail("join value");
        }
    }
    let done = Arc::new(AtomicUsize::new(0));
    for _ in 0..40 {
        let d = done.clone();
        // Dropped at once: detached, it frees its own mapping.
        drop(thread::spawn(move || {
            d.fetch_add(1, Ordering::SeqCst);
        }));
        // Under the task limit while the detached ones end.
        while Arc::strong_count(&done) > 8 {
            thread::yield_now();
        }
    }
    while Arc::strong_count(&done) > 1 {
        thread::sleep(Duration::from_millis(5));
    }
    check(done.load(Ordering::SeqCst) == 40, "detached threads");
}

#[unsafe(no_mangle)]
pub extern "C" fn main() {
    let cpus = thread::available_parallelism().map_or(0, |n| n.get());
    check(cpus >= 1, "available_parallelism");
    counters();
    thread_locals();
    sync();
    park_and_scope();
    many();
    println!("[ OK ] std thread ({cpus} CPUs)");
}
