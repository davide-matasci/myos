//! Reusable TLS client for myos userspace (mbedtls FFI).
//!
//! Architecture choice: a `user/tls` library (not Plan 9 `/net/tls` yet).
//! Smaller than teaching netfs/netd a PROTO_TLS, and structured so `/net/tls`
//! can wrap the same handshake later.

#![no_std]

use core::ffi::c_char;
use myos_user::{gettimeofday, read, write_fd};

unsafe extern "C" {
    fn myos_tls_conn_size() -> usize;
    fn myos_tls_handshake(conn: *mut u8, fd: i32, sni: *const c_char) -> i32;
    fn myos_tls_write(conn: *mut u8, buf: *const u8, len: usize) -> i32;
    fn myos_tls_read(conn: *mut u8, buf: *mut u8, len: usize) -> i32;
    fn myos_tls_close(conn: *mut u8);
}

/// Syscall bridge for platform.c
#[unsafe(no_mangle)]
pub unsafe extern "C" fn myos_tls_gettimeofday_sec(sec: *mut i64) -> i32 {
    match gettimeofday() {
        Some((s, _)) => {
            if !sec.is_null() {
                unsafe { *sec = s };
            }
            0
        }
        None => -1,
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn myos_tls_fd_read(fd: i32, buf: *mut u8, len: usize) -> i32 {
    if buf.is_null() || len == 0 {
        return 0;
    }
    let slice = unsafe { core::slice::from_raw_parts_mut(buf, len) };
    let n = read(fd as usize, slice);
    if n == usize::MAX {
        -1
    } else {
        n as i32
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn myos_tls_fd_write(fd: i32, buf: *const u8, len: usize) -> i32 {
    if buf.is_null() || len == 0 {
        return 0;
    }
    let slice = unsafe { core::slice::from_raw_parts(buf, len) };
    let n = write_fd(fd as usize, slice);
    if n == usize::MAX {
        -1
    } else {
        n as i32
    }
}


/// Page-aligned brk arena for mbedtls (see platform.c). Must run after
/// `myos_user::heap_init` so the bump allocator has observed the break.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn myos_tls_alloc_heap(len: usize) -> *mut u8 {
    myos_user::alloc::alloc_aligned(len, 4096)
}

/// Fixed-capacity TLS session storage (must hold `myos_tls_conn`).
pub struct TlsConn {
    raw: [u8; 96 * 1024],
    live: bool,
}

impl TlsConn {
    pub const fn new() -> Self {
        Self {
            raw: [0; 96 * 1024],
            live: false,
        }
    }

    /// Perform TLS handshake on an already-connected TCP data fd.
    /// `sni_host` must be a NUL-terminated hostname buffer.
    pub fn handshake(&mut self, fd: usize, sni_host_nul: &[u8]) -> Result<(), i32> {
        let need = unsafe { myos_tls_conn_size() };
        if need > self.raw.len() {
            return Err(-999);
        }
        self.raw[..need].fill(0);
        let sni = sni_host_nul.as_ptr() as *const c_char;
        let rc = unsafe { myos_tls_handshake(self.raw.as_mut_ptr(), fd as i32, sni) };
        if rc != 0 {
            return Err(rc);
        }
        self.live = true;
        Ok(())
    }

    pub fn write_all(&mut self, mut buf: &[u8]) -> Result<(), i32> {
        while !buf.is_empty() {
            let n = unsafe { myos_tls_write(self.raw.as_mut_ptr(), buf.as_ptr(), buf.len()) };
            if n < 0 {
                return Err(n);
            }
            if n == 0 {
                continue;
            }
            buf = &buf[n as usize..];
        }
        Ok(())
    }

    pub fn read(&mut self, buf: &mut [u8]) -> Result<usize, i32> {
        let n = unsafe { myos_tls_read(self.raw.as_mut_ptr(), buf.as_mut_ptr(), buf.len()) };
        if n < 0 {
            Err(n)
        } else {
            Ok(n as usize)
        }
    }

    pub fn close(&mut self) {
        if self.live {
            unsafe { myos_tls_close(self.raw.as_mut_ptr()) };
            self.live = false;
        }
    }
}

impl Drop for TlsConn {
    fn drop(&mut self) {
        self.close();
    }
}
