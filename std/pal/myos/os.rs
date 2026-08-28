//! Raw OS entry points used by `std` before full I/O is wired.

use crate::io;
use crate::num::NonZeroU32;

const SYS_WRITE: usize = 0;
const SYS_EXIT: usize = 1;
const SYS_OPEN: usize = 2;
const SYS_READ: usize = 3;
const SYS_CLOSE: usize = 4;

pub fn errno() -> i32 {
    0
}

pub fn set_errno(_e: i32) {}

pub fn abort_internal() -> ! {
    unsafe {
        core::arch::asm!("syscall", in("rax") SYS_EXIT, options(noreturn));
    }
}

pub fn write(_fd: i32, buf: &[u8]) -> io::Result<usize> {
    let n = unsafe {
        let ret: usize;
        core::arch::asm!(
            "syscall",
            in("rax") SYS_WRITE,
            in("rdi") buf.as_ptr() as usize,
            in("rsi") buf.len(),
            lateout("rax") ret,
            out("rcx") _,
            out("r11") _,
            options(nostack),
        );
        ret
    };
    if n == usize::MAX {
        Err(io::Error::last_os_error())
    } else {
        Ok(n)
    }
}

pub fn read(fd: i32, buf: &mut [u8]) -> io::Result<usize> {
    let n = unsafe {
        let ret: usize;
        core::arch::asm!(
            "syscall",
            in("rax") SYS_READ,
            in("rdi") fd as usize,
            in("rsi") buf.as_mut_ptr() as usize,
            in("rdx") buf.len(),
            lateout("rax") ret,
            out("rcx") _,
            out("r11") _,
            options(nostack),
        );
        ret
    };
    if n == usize::MAX {
        Err(io::Error::last_os_error())
    } else {
        Ok(n)
    }
}

pub fn open(path: &CStr) -> io::Result<OwnedFd> {
    let ret = unsafe {
        let ret: usize;
        core::arch::asm!(
            "syscall",
            in("rax") SYS_OPEN,
            in("rdi") path.as_ptr() as usize,
            in("rsi") path.to_bytes().len(),
            lateout("rax") ret,
            out("rcx") _,
            out("r11") _,
            options(nostack),
        );
        ret
    };
    if ret == usize::MAX {
        Err(io::Error::last_os_error())
    } else {
        Ok(OwnedFd { fd: ret as i32 })
    }
}

pub struct OwnedFd {
    fd: i32,
}

impl OwnedFd {
    pub fn into_raw_fd(self) -> i32 {
        let fd = self.fd;
        core::mem::forget(self);
        fd
    }
}

impl Drop for OwnedFd {
    fn drop(&mut self) {
        unsafe {
            core::arch::asm!(
                "syscall",
                in("rax") SYS_CLOSE,
                in("rdi") self.fd as usize,
                out("rcx") _,
                out("r11") _,
                options(nostack),
            );
        }
    }
}

use crate::ffi::CStr;

pub fn unsupported<T>(_name: &str) -> io::Result<T> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "myos PAL stub",
    ))
}

pub fn isatty(_fd: i32) -> bool {
    false
}

pub fn exit(code: i32) -> ! {
    let _ = code;
    unsafe {
        core::arch::asm!("syscall", in("rax") SYS_EXIT, options(noreturn));
    }
}

pub fn getpid() -> NonZeroU32 {
    NonZeroU32::new(1).unwrap()
}
