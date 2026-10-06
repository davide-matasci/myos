use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicUsize, Ordering};

const MAX_ARGS: usize = 16;
const MAX_ARG_LEN: usize = 256;

/// The arguments, copied at entry by [`load_argv`] before anything reads
/// them, then only read: `ARGC` publishes them.
struct Argv(UnsafeCell<Store>);

struct Store {
    bytes: [[u8; MAX_ARG_LEN]; MAX_ARGS],
    lens: [usize; MAX_ARGS],
}

// SAFETY: written once, by `load_argv` at entry (before a thread exists),
// and read only below the count `ARGC` publishes after that.
unsafe impl Sync for Argv {}

static ARGC: AtomicUsize = AtomicUsize::new(0);
static ARGV: Argv = Argv(UnsafeCell::new(Store {
    bytes: [[0; MAX_ARG_LEN]; MAX_ARGS],
    lens: [0; MAX_ARGS],
}));

/// x86: read argv copied from the stack at `_start` (see [`crate::x86_start`]).
pub unsafe fn init_from_sp(sp: usize) {
    unsafe {
        load_argv(
            *(sp as *const usize),
            (sp + core::mem::size_of::<usize>()) as *const usize,
        );
    }
}

/// x86 legacy entry — prefer [`init_from_sp`] via [`crate::x86_start`].
#[cfg(target_arch = "x86_64")]
pub unsafe fn init_from_stack() {
    let sp: usize;
    unsafe {
        core::arch::asm!("mov {}, rsp", out(reg) sp, options(nomem, nostack));
        init_from_sp(sp);
    }
}

/// AArch64: kernel passes argc/argv in x0/x1 across `eret`.
pub unsafe fn init_from_regs(argc: usize, argv: *const usize) {
    unsafe { load_argv(argc, argv) };
}

/// x86 SysV entry from [`crate::x86_start`] (`rdi`=argc, `rsi`=argv).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn init_argv_sysv(argc: usize, argv: *const usize) {
    unsafe { load_argv(argc, argv) };
}

/// Copy `argc` C strings of `argv` (fewer at a null entry) into `ARGV`.
///
/// # Safety
///
/// `argv` holds `argc` pointers to NUL-terminated strings, and this runs
/// once, at entry, before anything reads the arguments.
unsafe fn load_argv(argc: usize, argv: *const usize) {
    // SAFETY: nothing reads `ARGV` yet (see above).
    let store = unsafe { &mut *ARGV.0.get() };
    let mut n = 0;
    while n < argc.min(MAX_ARGS) {
        let p = unsafe { *argv.add(n) } as *const u8;
        if p.is_null() {
            break;
        }
        let mut len = 0usize;
        while len < MAX_ARG_LEN && unsafe { *p.add(len) } != 0 {
            len += 1;
        }
        store.bytes[n][..len].copy_from_slice(unsafe { core::slice::from_raw_parts(p, len) });
        store.lens[n] = len;
        n += 1;
    }
    ARGC.store(n, Ordering::Release);
}

pub fn argc() -> usize {
    ARGC.load(Ordering::Acquire)
}

pub fn arg(i: usize) -> Option<&'static [u8]> {
    if i >= argc() {
        return None;
    }
    // SAFETY: the entries below `ARGC` are written and no longer change.
    let store = unsafe { &*ARGV.0.get() };
    Some(&store.bytes[i][..store.lens[i]])
}
