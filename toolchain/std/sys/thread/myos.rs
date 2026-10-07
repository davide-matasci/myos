//! Threads on myos (`docs/threads.md`): native `thread_spawn` threads of
//! the calling process, each on a stack of its own mapping.
//!
//! A thread's mapping is a guard page, its stack and a top page holding its
//! TLS table (its thread pointer, `thread_local::key`) and the [`Top`] the
//! thread and its `Thread` handle share. Whoever is last frees it: the
//! joiner once the thread has ended, or the thread itself when its handle
//! was dropped (detached) first. A thread ends in [`exit`], which neither
//! touches its stack again once a joiner may free it nor lets a signal
//! handler run on it.

use crate::num::NonZero;
use crate::sync::atomic::Atomic;
use crate::sync::atomic::AtomicU32;
use crate::sync::atomic::Ordering::{AcqRel, Acquire};
use crate::sys::myos::abi;
use crate::sys::thread_local::key::{Table, init_table, run_dtors};
use crate::thread::ThreadInit;
use crate::time::{Duration, SystemTime};
use crate::{io, mem, ptr};

const PAGE: usize = 4096;

/// `std::thread::spawn`'s stack size (`RUST_MIN_STACK` is not read: no
/// environment yet). Pages are only taken when touched.
pub const DEFAULT_MIN_STACK_SIZE: usize = 1 << 20;

/// [`Top::state`]: the thread runs and its handle is held,
const RUNNING: u32 = 0;
/// the thread has ended (its handle frees the mapping),
const EXITED: u32 = 1;
/// or its handle was dropped (the thread frees the mapping).
const DETACHED: u32 = 2;

/// The top page of a thread's mapping; the thread pointer points at it.
#[repr(C)]
struct Top {
    tls: Table,
    state: Atomic<u32>,
    init: *mut ThreadInit,
    /// The whole mapping.
    base: usize,
    len: usize,
}

const _: () = assert!(mem::size_of::<Top>() <= PAGE);

pub struct Thread {
    top: *mut Top,
}

unsafe impl Send for Thread {}
unsafe impl Sync for Thread {}

impl Thread {
    // unsafe: see thread::Builder::spawn_unchecked for safety requirements
    pub unsafe fn new(stack: usize, init: Box<ThreadInit>) -> io::Result<Thread> {
        let stack = stack.max(16 * 1024).next_multiple_of(PAGE);
        let len = PAGE + stack + PAGE;
        let base = abi::mmap_anon(len).ok_or(io::const_error!(
            io::ErrorKind::OutOfMemory,
            "no memory for the thread's stack"
        ))?;
        // A stack overflow faults instead of running into another mapping.
        abi::mprotect_none(base, PAGE);
        let stack_top = base + PAGE + stack;
        let top = ptr::with_exposed_provenance_mut::<Top>(stack_top);
        // The mapping is zeroed: the TLS table empty, the state RUNNING.
        unsafe {
            init_table(&raw mut (*top).tls);
            (*top).init = Box::into_raw(init);
            (*top).base = base;
            (*top).len = len;
        }
        if abi::thread_spawn(start as usize, stack_top, stack_top, stack_top).is_none() {
            unsafe { drop(Box::from_raw((*top).init)) };
            abi::munmap(base, len);
            return Err(io::const_error!(io::ErrorKind::WouldBlock, "no task for the thread"));
        }
        Ok(Thread { top })
    }

    pub fn join(self) {
        let top = self.top;
        mem::forget(self);
        let state = unsafe { &(*top).state };
        while state.load(Acquire) == RUNNING {
            abi::wait_addr(state.as_ptr(), RUNNING, 0);
        }
        // Ended: it does not touch its mapping again.
        unsafe { abi::munmap((*top).base, (*top).len) };
    }
}

impl Drop for Thread {
    /// Detach: the thread frees its mapping when it ends, or, if it has
    /// already, the handle does.
    fn drop(&mut self) {
        let state = unsafe { &(*self.top).state };
        if state.swap(DETACHED, AcqRel) == EXITED {
            unsafe { abi::munmap((*self.top).base, (*self.top).len) };
        }
    }
}

/// A new thread's entry (`thread_spawn`), `top` its [`Top`].
extern "C" fn start(top: usize) -> ! {
    let top = ptr::with_exposed_provenance_mut::<Top>(top);
    run(unsafe { Box::from_raw((*top).init) });
    unsafe { run_dtors() };
    // From here a joiner may free the stack: a signal frame must not land
    // on it.
    abi::block_signals();
    unsafe { exit(&raw const (*top).state, (*top).base, (*top).len) }
}

// Not inlined: no deallocation of the thread's may move past the TLS
// destructors (see the Xous version).
#[inline(never)]
fn run(init: Box<ThreadInit>) {
    let rust_start = init.init();
    rust_start();
}

/// Set `state` to EXITED and end the thread: wake the joiner, or unmap
/// `base..base + len` (the thread's own mapping, `state` and the stack
/// included) when the handle was dropped. Registers only.
unsafe fn exit(state: *const Atomic<u32>, base: usize, len: usize) -> ! {
    unsafe {
        #[cfg(target_arch = "x86_64")]
        core::arch::asm!(
            "mov eax, {exited}",
            "xchg dword ptr [r12], eax",
            "cmp eax, {detached}",
            "je 2f",
            "mov rdi, r12",
            "mov esi, 1",
            "mov eax, {wake}",
            "syscall",
            "jmp 3f",
            "2:",
            "mov rdi, r13",
            "mov rsi, r14",
            "mov eax, {munmap}",
            "syscall",
            "3:",
            "xor edi, edi",
            "mov eax, {exit}",
            "syscall",
            "ud2",
            exited = const EXITED,
            detached = const DETACHED,
            wake = const abi::SYS_WAKE_ADDR,
            munmap = const abi::SYS_MUNMAP,
            exit = const abi::SYS_THREAD_EXIT,
            in("r12") state,
            in("r13") base,
            in("r14") len,
            options(noreturn, nostack),
        );
        #[cfg(target_arch = "aarch64")]
        core::arch::asm!(
            "mov w9, #{exited}",
            "1:",
            "ldaxr w10, [x20]",
            "stlxr w11, w9, [x20]",
            "cbnz w11, 1b",
            "cmp w10, #{detached}",
            "b.eq 2f",
            "mov x0, x20",
            "mov x1, #1",
            "mov x8, #{wake}",
            "svc #0",
            "b 3f",
            "2:",
            "mov x0, x21",
            "mov x1, x22",
            "mov x8, #{munmap}",
            "svc #0",
            "3:",
            "mov x0, #0",
            "mov x8, #{exit}",
            "svc #0",
            "udf #0",
            exited = const EXITED,
            detached = const DETACHED,
            wake = const abi::SYS_WAKE_ADDR,
            munmap = const abi::SYS_MUNMAP,
            exit = const abi::SYS_THREAD_EXIT,
            in("x20") state,
            in("x21") base,
            in("x22") len,
            options(noreturn, nostack),
        );
        #[cfg(target_arch = "riscv64")]
        core::arch::asm!(
            "li t1, {exited}",
            "amoswap.w.aqrl t0, t1, (s2)",
            "li t1, {detached}",
            "beq t0, t1, 2f",
            "mv a0, s2",
            "li a1, 1",
            "li a7, {wake}",
            "ecall",
            "j 3f",
            "2:",
            "mv a0, s3",
            "mv a1, s4",
            "li a7, {munmap}",
            "ecall",
            "3:",
            "li a0, 0",
            "li a7, {exit}",
            "ecall",
            "unimp",
            exited = const EXITED,
            detached = const DETACHED,
            wake = const abi::SYS_WAKE_ADDR,
            munmap = const abi::SYS_MUNMAP,
            exit = const abi::SYS_THREAD_EXIT,
            in("s2") state,
            in("s3") base,
            in("s4") len,
            options(noreturn, nostack),
        );
    }
}

/// The CPUs: the `cpuN` lines of `/proc/cpu`.
pub fn available_parallelism() -> io::Result<NonZero<usize>> {
    let text = crate::fs::read("/proc/cpu")?;
    let n = text.split(|&b| b == b'\n').filter(|l| l.starts_with(b"cpu")).count();
    NonZero::new(n).ok_or(io::Error::UNKNOWN_THREAD_COUNT)
}

pub fn current_os_id() -> Option<u64> {
    Some(abi::gettid() as u64)
}

pub fn yield_now() {
    abi::yield_now();
}

pub fn sleep(dur: Duration) {
    let start = SystemTime::now();
    let mut left = dur;
    loop {
        if abi::nanosleep(left) {
            return;
        }
        // Interrupted: sleep what is left, by the wall clock.
        let slept = SystemTime::now().duration_since(start).unwrap_or(Duration::ZERO);
        match dur.checked_sub(slept) {
            Some(l) if !l.is_zero() => left = l,
            _ => return,
        }
    }
}
