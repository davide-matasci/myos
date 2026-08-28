const MAX_ARGS: usize = 16;

static mut ARGC: usize = 0;
static mut ARGV: [*const u8; MAX_ARGS] = [core::ptr::null(); MAX_ARGS];
static mut ARGV_LEN: [usize; MAX_ARGS] = [0; MAX_ARGS];

/// x86: argc/argv on the user stack at `_start`.
#[cfg(target_arch = "x86_64")]
pub unsafe fn init_from_stack() {
    let sp: usize;
    core::arch::asm!("mov {}, rsp", out(reg) sp, options(nomem, nostack));
    load_argv(*(sp as *const usize), (sp + core::mem::size_of::<usize>()) as *const usize);
}

/// AArch64: kernel passes argc/argv in x0/x1 across `eret`.
pub unsafe fn init_from_regs(argc: usize, argv: *const usize) {
    load_argv(argc, argv);
}

unsafe fn load_argv(argc: usize, argv: *const usize) {
    ARGC = argc.min(MAX_ARGS);
    for i in 0..ARGC {
        let p = *argv.add(i) as *const u8;
        ARGV[i] = p;
        let mut len = 0usize;
        while len < 256 {
            if *p.add(len) == 0 {
                break;
            }
            len += 1;
        }
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
        Some(core::slice::from_raw_parts(ARGV[i], len))
    }
}
