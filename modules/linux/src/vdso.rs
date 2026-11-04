//! The vDSO (`vdso/`, docs/linux-compat.md "vDSO"): one page of code mapped
//! into every Linux process at exec, whose `clock_gettime` reads the
//! monotonic clocks from the counter the kernel's monotonic clock runs on,
//! without a syscall. musl finds it through `AT_SYSINFO_EHDR`.
//!
//! The image is built by `build.rs`. Its clock block (`myos_clock` in
//! `vdso.c`: a magic, the counter's value at nanosecond 0, its rate) is
//! filled in once, here, from `KernelApi::user_clock`; each exec maps a
//! copy, as the signal trampoline is (`signal::trampoline`).

use alloc::vec::Vec;

use myos_abi::Lock;

use super::sys::put;
use crate::k::user;

static IMAGE: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/vdso.so"));

/// `myos_clock.magic` in `vdso.c`: "myosclk".
const MAGIC: u64 = 0x006b_6c63_736f_796d;

/// The image with its clock block filled in; `None` when the kernel's
/// counter is not one user mode can read (every exec then goes without:
/// `AT_SYSINFO_EHDR` stays 0 and libc makes the syscalls).
static READY: Lock<Option<Vec<u8>>> = Lock::new(None);

/// At module load: fill the clock block in.
pub fn init() {
    let Some((base, hz)) = crate::k::api().user_clock() else { return };
    // The vDSO's `(c % hz) * 10^9` must not overflow (the kernel computes
    // it in 128 bits): every counter here is a few GHz at most.
    if hz == 0 || hz > u64::MAX / 1_000_000_000 || IMAGE.len() > user::PAGE {
        return;
    }
    let mut image = IMAGE.to_vec();
    let Some(at) = image.windows(8).position(|w| w == MAGIC.to_le_bytes()) else { return };
    if at % 8 != 0 || at + 24 > image.len() {
        return;
    }
    image[at + 8..at + 16].copy_from_slice(&base.to_le_bytes());
    image[at + 16..at + 24].copy_from_slice(&hz.to_le_bytes());
    *READY.lock() = Some(image);
}

/// `PersonalityOps::map_vdso`: a copy of the image mapped read-only and
/// executable into the calling process, which just exec'd; its address,
/// or 0 for none.
pub fn map() -> usize {
    const PROT_READ: usize = 1;
    const PROT_WRITE: usize = 2;
    const PROT_EXEC: usize = 4;
    const MAP_PRIVATE: usize = 2;
    const MAP_ANON: usize = 0x20;
    let ready = READY.lock();
    let Some(image) = ready.as_ref() else { return 0 };
    let va = user::do_mmap(0, user::PAGE, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANON, -1, 0);
    if va >= crate::k::signal::SYSERR_LOWEST {
        return 0;
    }
    if put(va, image).is_err() || user::sys_mprotect(va, user::PAGE, PROT_READ | PROT_EXEC) != 0 {
        user::sys_munmap(va, user::PAGE);
        return 0;
    }
    va
}
