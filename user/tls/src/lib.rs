//! Reusable TLS client for myos userspace: LibreSSL's libtls (ports/libressl)
//! over a connected `/net` TCP data fd, through `src/platform.c`.
//!
//! Architecture choice: a `user/tls` library (not Plan 9 `/net/tls` yet).
//! Smaller than teaching netfs/netd a PROTO_TLS, and structured so `/net/tls`
//! can wrap the same handshake later. The peer is verified against the
//! Mozilla CA bundle curl ships as `/lib/cacert.pem`.

#![no_std]
#![deny(unsafe_op_in_unsafe_fn)]

use core::ffi::{CStr, c_char, c_void};
use myos_user::{read, write_fd};

unsafe extern "C" {
    fn myos_tls_handshake(fd: i32, sni: *const c_char) -> *mut c_void;
    fn myos_tls_ready(conn: *const c_void) -> i32;
    fn myos_tls_error(conn: *const c_void) -> *const c_char;
    fn myos_tls_write(conn: *mut c_void, buf: *const u8, len: usize) -> i32;
    fn myos_tls_read(conn: *mut c_void, buf: *mut u8, len: usize) -> i32;
    fn myos_tls_close(conn: *mut c_void);
}

/// Syscall bridge for platform.c: a read of the data fd, 0 when nothing came
/// yet, -1 on an error.
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

/// A TLS connection: nothing until [`TlsConn::handshake`], the C side's
/// connection (its `struct tls`, on the C heap) after.
pub struct TlsConn {
    raw: *mut c_void,
    live: bool,
}

impl TlsConn {
    pub const fn new() -> Self {
        Self {
            raw: core::ptr::null_mut(),
            live: false,
        }
    }

    /// Perform the TLS handshake on an already-connected TCP data fd.
    /// `sni_host_nul` must be a NUL-terminated hostname buffer. On failure
    /// [`TlsConn::error`] says why.
    pub fn handshake(&mut self, fd: usize, sni_host_nul: &[u8]) -> Result<(), ()> {
        self.close();
        let sni = sni_host_nul.as_ptr() as *const c_char;
        self.raw = unsafe { myos_tls_handshake(fd as i32, sni) };
        if self.raw.is_null() {
            return Err(());
        }
        if unsafe { myos_tls_ready(self.raw) } == 0 {
            return Err(());
        }
        self.live = true;
        Ok(())
    }

    /// The last failure's reason (libtls's message), empty when none.
    pub fn error(&self) -> &[u8] {
        if self.raw.is_null() {
            return b"out of memory";
        }
        let p = unsafe { myos_tls_error(self.raw) };
        if p.is_null() {
            return b"";
        }
        unsafe { CStr::from_ptr(p) }.to_bytes()
    }

    pub fn write_all(&mut self, buf: &[u8]) -> Result<(), ()> {
        if !self.live {
            return Err(());
        }
        let n = unsafe { myos_tls_write(self.raw, buf.as_ptr(), buf.len()) };
        if n < 0 || n as usize != buf.len() {
            return Err(());
        }
        Ok(())
    }

    /// Bytes read, 0 when nothing came yet or the peer closed.
    pub fn read(&mut self, buf: &mut [u8]) -> Result<usize, ()> {
        if !self.live {
            return Err(());
        }
        let n = unsafe { myos_tls_read(self.raw, buf.as_mut_ptr(), buf.len()) };
        if n < 0 {
            Err(())
        } else {
            Ok(n as usize)
        }
    }

    /// close_notify when the connection is up, and everything freed.
    pub fn close(&mut self) {
        if !self.raw.is_null() {
            unsafe { myos_tls_close(self.raw) };
            self.raw = core::ptr::null_mut();
        }
        self.live = false;
    }
}

impl Default for TlsConn {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for TlsConn {
    fn drop(&mut self) {
        self.close();
    }
}
