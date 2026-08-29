const MAX_ARGS: usize = 16;
const MAX_ARG_LEN: usize = 256;

static mut ARGC: usize = 0;
static mut ARGV: [[u8; MAX_ARG_LEN]; MAX_ARGS] = [[0; MAX_ARG_LEN]; MAX_ARGS];
static mut ARGV_LEN: [usize; MAX_ARGS] = [0; MAX_ARGS];

/// x86: read argv copied from the stack at `_start` (see [`crate::x86_start`]).
pub unsafe fn init_from_sp(sp: usize) {
    load_argv(
        *(sp as *const usize),
        (sp + core::mem::size_of::<usize>()) as *const usize,
    );
}

/// x86 legacy entry — prefer [`init_from_sp`] via [`crate::x86_start`].
#[cfg(target_arch = "x86_64")]
pub unsafe fn init_from_stack() {
    let sp: usize;
    core::arch::asm!("mov {}, rsp", out(reg) sp, options(nomem, nostack));
    init_from_sp(sp);
}

/// AArch64: kernel passes argc/argv in x0/x1 across `eret`.
pub unsafe fn init_from_regs(argc: usize, argv: *const usize) {
    load_argv(argc, argv);
}

unsafe fn load_argv(argc: usize, argv: *const usize) {
    ARGC = argc.min(MAX_ARGS);
    for i in 0..ARGC {
        let p = *argv.add(i) as *const u8;
        if p.is_null() {
            ARGC = i;
            break;
        }
        let mut len = 0usize;
        while len < MAX_ARG_LEN {
            if *p.add(len) == 0 {
                break;
            }
            len += 1;
        }
        ARGV[i][..len].copy_from_slice(core::slice::from_raw_parts(p, len));
        ARGV_LEN[i] = len;
    }
}

pub fn argc() -> usize {
    unsafe { ARGC }
}

pub fn arg(i: usize) -> Option<&'static [u8]> {
    if i >= unsafe { ARGC } {
        return None;
    }
    unsafe {
        let len = ARGV_LEN[i];
        Some(&ARGV[i][..len])
    }
}
