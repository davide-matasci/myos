//! Linux ABI constants and the conversions to and from the native ones
//! (errno values, signal numbers, `struct stat`, `struct dirent64`).

pub const EPERM: usize = 1;
pub const ENOENT: usize = 2;
pub const ESRCH: usize = 3;
pub const EINTR: usize = 4;
pub const EIO: usize = 5;
pub const ENXIO: usize = 6;
pub const E2BIG: usize = 7;
pub const EBADF: usize = 9;
pub const ECHILD: usize = 10;
pub const ENOMEM: usize = 12;
pub const EFAULT: usize = 14;
pub const EEXIST: usize = 17;
pub const ENOTDIR: usize = 20;
pub const EINVAL: usize = 22;
pub const EMFILE: usize = 24;
pub const ENOTTY: usize = 25;
pub const ESPIPE: usize = 29;
pub const ERANGE: usize = 34;
pub const ENOSYS: usize = 38;
pub const ENODEV: usize = 19;

/// A Linux error return: `-errno` in the result register.
pub const fn err(errno: usize) -> usize {
    errno.wrapping_neg()
}

/// Map a native result to a Linux one. The native layer returns a few
/// sentinels just below `usize::MAX` instead of errno values; the generic
/// failure (`usize::MAX`) becomes `generic`, the syscall's most likely errno.
pub fn result(ret: usize, generic: usize) -> usize {
    match ret {
        usize::MAX => err(generic),
        x if x == usize::MAX - 1 => err(EIO),
        x if x == usize::MAX - 2 => err(ENXIO),
        x if x == crate::signal::SYSERR_EINTR => err(EINTR),
        x => x,
    }
}

/// Linux signal number -> native (newlib / BSD numbering), 0 if none.
pub fn sig_from_linux(sig: usize) -> u32 {
    const MAP: [u32; 32] = [
        0, 1, 2, 3, 4, 5, 6, 10, 8, 9, // .. KILL; Linux BUS=7 -> 10
        30, 11, 31, 13, 14, 15, // USR1, SEGV, USR2, PIPE, ALRM, TERM
        0,  // STKFLT
        20, 19, 17, 18, 21, 22, 16, // CHLD CONT STOP TSTP TTIN TTOU URG
        24, 25, 26, 27, 28, 23, // XCPU XFSZ VTALRM PROF WINCH IO
        0,  // PWR
        12, // SYS
    ];
    MAP.get(sig).copied().unwrap_or(0)
}

/// Native signal number -> Linux.
pub fn sig_to_linux(sig: u32) -> usize {
    (1..32).find(|&l| sig_from_linux(l) == sig).unwrap_or(0)
}

/// A Linux `sigset_t` (bit `n-1` = signal `n`) as a native mask (bit `n`).
pub fn mask_from_linux(set: u64) -> u32 {
    let mut out = 0u32;
    for l in 1..32 {
        if set & (1 << (l - 1)) != 0 {
            let n = sig_from_linux(l);
            if n != 0 {
                out |= 1 << n;
            }
        }
    }
    out
}

pub fn mask_to_linux(mask: u32) -> u64 {
    let mut out = 0u64;
    for n in 1..32u32 {
        if mask & (1 << n) != 0 {
            let l = sig_to_linux(n);
            if l != 0 {
                out |= 1 << (l - 1);
            }
        }
    }
    out
}

pub const DT_UNKNOWN: u8 = 0;
pub const DT_FIFO: u8 = 1;
pub const DT_CHR: u8 = 2;
pub const DT_DIR: u8 = 4;
pub const DT_REG: u8 = 8;
pub const DT_LNK: u8 = 10;

pub fn dtype(mode: u32) -> u8 {
    match mode & 0o170000 {
        0o010000 => DT_FIFO,
        0o020000 => DT_CHR,
        0o040000 => DT_DIR,
        0o100000 => DT_REG,
        0o120000 => DT_LNK,
        _ => DT_UNKNOWN,
    }
}

/// Append one `struct linux_dirent64` to `out` if it fits in `cap`.
pub fn push_dirent(out: &mut alloc::vec::Vec<u8>, cap: usize, ino: u64, off: u64, ty: u8, name: &[u8]) -> bool {
    let reclen = (19 + name.len() + 1).next_multiple_of(8);
    if out.len() + reclen > cap {
        return false;
    }
    out.extend_from_slice(&ino.to_le_bytes());
    out.extend_from_slice(&off.to_le_bytes());
    out.extend_from_slice(&(reclen as u16).to_le_bytes());
    out.push(ty);
    out.extend_from_slice(name);
    out.resize(out.len() + (reclen - 19 - name.len()), 0);
    true
}
