//! argc/argv from the SysV `_start` stack.

use crate::ffi::CStr;
use crate::sync::Once;

static INIT: Once = Once::new();
static mut ARGC: isize = 0;
static mut ARGV: *const *const u8 = core::ptr::null();

pub fn init() {
    INIT.call_once(|| unsafe {
        let sp: usize;
        core::arch::asm!("mov {}, rsp", out(reg) sp, options(nomem, nostack));
        ARGC = *(sp as *const isize);
        ARGV = (sp + core::mem::size_of::<isize>()) as *const *const u8;
    });
}

pub fn args() -> (*const isize, *const *const u8) {
    init();
    unsafe { (&raw const ARGC, ARGV) }
}

pub fn env() -> *const *const u8 {
    init();
    unsafe {
        if ARGC <= 0 {
            return core::ptr::null();
        }
        ARGV.add(ARGC as usize + 1)
    }
}

pub fn c_args() -> *const *const u8 {
    init();
    unsafe { ARGV }
}

pub fn c_environ() -> *const *const u8 {
    env()
}

pub unsafe fn init_environment(_env: *const *const u8) {}

pub fn page_size() -> usize {
    4096
}

pub fn getcwd(_buf: &mut [u8]) -> crate::io::Result<()> {
    Err(crate::io::Error::new(
        crate::io::ErrorKind::Unsupported,
        "getcwd",
    ))
}

pub fn chdir(_path: &CStr) -> crate::io::Result<()> {
    Err(crate::io::Error::new(
        crate::io::ErrorKind::Unsupported,
        "chdir",
    ))
}
