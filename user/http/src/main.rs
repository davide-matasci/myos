#![no_std]
#![no_main]

use myos_user::{close, exit, exit_code, open, open_flags, read, write, write_fd, O_RDWR, O_WRONLY};

myos_user::x86_start!(main);

#[cfg(any(target_arch = "aarch64", target_arch = "riscv64"))]
#[unsafe(no_mangle)]
pub extern "C" fn _start(argc: usize, argv: *const usize) -> ! {
    unsafe { myos_user::args::init_from_regs(argc, argv) };
    main()
}

/// Exit codes baked into main — reuse across similar tools.
const MAX_POLLS: usize = 200_000;
const STATUS_POLLS: usize = 200_000;
const DATA_POLLS: usize = 400_000;
const BUF: usize = 1024;

fn usage() -> ! {
    write(b"usage: http <ipv4> [port] [path]\n");
    write(b"  default port 80, default path /\n");
    exit_code(1);
}

fn fail(msg: &[u8]) -> ! {
    write(msg);
    exit_code(1);
}

/// Parse dotted-quad IPv4. Returns true iff the byte slice has exactly 4 parts,
/// each 0–255, separated by single dots with no leading zeros >1 digit.
fn parse_ipv4(s: &[u8]) -> bool {
    let mut i = 0usize;
    let mut parts = 0;
    while i < s.len() && parts < 4 {
        if !s[i].is_ascii_digit() {
            return false;
        }
        let mut n = 0u16;
        let mut digits = 0u8;
        while i < s.len() && s[i].is_ascii_digit() {
            digits += 1;
            if digits > 3 {
                return false;
            }
            n = n * 10 + (s[i] - b'0') as u16;
            if n > 255 {
                return false;
            }
            i += 1;
        }
        parts += 1;
        if parts != 4 {
            // Must see a dot separator (unless we're already at end, which fails)
            if i >= s.len() || s[i] != b'.' {
                return false;
            }
            i += 1; // skip dot
        }
    }
    parts == 4 && i == s.len()
}

/// Parse a port number string. Returns true iff every char is a digit,
/// value is in 1..=65535, and the string is non-empty.
fn parse_port(s: &[u8]) -> bool {
    if s.is_empty() || s.len() > 5 {
        return false;
    }
    let mut n = 0u32;
    for &b in s {
        if !b.is_ascii_digit() {
            return false;
        }
        n = n.checked_mul(10).unwrap_or(u32::MAX).saturating_add((b - b'0') as u32);
    }
    0 < n && n <= 65535
}

/// Parse a plain unsigned decimal id from a read buffer.
fn parse_id(buf: &[u8]) -> Option<u16> {
    let mut n = 0u32;
    let mut any = false;
    for &b in buf {
        match b {
            b'\n' | b'\r' | b' ' | b'\0' => break,
            _ => {
                if !b.is_ascii_digit() {
                    return None;
                }
                any = true;
                n = n.checked_mul(10)?.checked_add((b - b'0') as u32)?;
                if n > 65535 {
                    return None;
                }
            }
        }
    }
    if any { Some(n as u16) } else { None }
}

/// Format a u16 as decimal into a buffer, return chars written.
fn put_dec(buf: &mut [u8], mut n: u16) -> usize {
    let mut tmp = [0u8; 6];
    let mut i = tmp.len();
    if n == 0 {
        i -= 1;
        tmp[i] = b'0';
    } else {
        while n != 0 && i > 0 {
            i -= 1;
            tmp[i] = b'0' + (n % 10) as u8;
            n /= 10;
        }
    }
    let digits = &tmp[i..];
    buf[..digits.len()].copy_from_slice(digits);
    digits.len()
}

/// Build a path like /net/tcp/<id>/ctl (or /data /status).
fn conv_path(out: &mut [u8], id: u16, leaf: &[u8]) -> usize {
    let prefix = b"/net/tcp/";
    let mut n = 0usize;
    out[n..n + prefix.len()].copy_from_slice(prefix);
    n += prefix.len();
    n += put_dec(&mut out[n..], id);
    out[n] = b'/';
    n += 1;
    out[n..n + leaf.len()].copy_from_slice(leaf);
    n += leaf.len();
    n
}

/// Does a byte slice contain the given needle anywhere (windowed check)?
fn buf_has(hay: &[u8], needle: &[u8]) -> bool {
    hay.windows(needle.len()).any(|w| w == needle)
}

/// Write a `connect ip!port` ctl command for a TCP conversation.
fn connect_ctl(ip: &[u8], port: u16, out: &mut [u8]) -> usize {
    let prefix = b"connect ";
    let mut n = prefix.len();
    out[..n].copy_from_slice(prefix);
    out[n..n + ip.len()].copy_from_slice(ip);
    n += ip.len();
    out[n] = b'!';
    n += 1;
    n += put_dec(&mut out[n..], port);
    n
}

fn main() -> ! {
    let Some(ip_str) = myos_user::arg(1) else {
        usage();
    };
    let ip_bytes = ip_str.as_bytes();
    if !parse_ipv4(ip_bytes) {
        fail(b"bad ipv4\n");
    }
    // Parse optional port argument.
    let port: u16 = match myos_user::arg(2) {
        Some(p) => {
            let p_bytes = p.as_bytes();
            if !parse_port(p_bytes) {
                fail(b"bad port\n");
            }
            let mut n = 0u32;
            for &b in p_bytes {
                n = n * 10 + (b - b'0') as u32;
            }
            n as u16
        }
        None => 80,
    };
    // Parse optional path argument (must start with '/').
    let path: &[u8] = match myos_user::arg(3) {
        Some(p) => {
            let p_bytes = p.as_bytes();
            if p_bytes.is_empty() || p_bytes[0] != b'/' {
                fail(b"path must start with /\n");
            }
            p_bytes
        }
        None => b"/",
    };

    // ------------------------------------------------------------------
    // Step 1: clone a TCP conversation from /net/tcp/clone
    // ------------------------------------------------------------------
    let Some(clone) = open(b"/net/tcp/clone") else {
        fail(b"open /net/tcp/clone fail\n");
    };
    let mut idbuf = [0u8; 16];
    let n = read(clone, &mut idbuf);
    close(clone);
    if n == 0 || n == usize::MAX {
        fail(b"read clone fail\n");
    }
    let Some(id) = parse_id(&idbuf[..n]) else {
        fail(b"clone id fail\n");
    };

    // ------------------------------------------------------------------
    // Step 2: ctl — connect ip!port
    // ------------------------------------------------------------------
    let mut cbuf = [0u8; 40];
    let cn = conv_path(&mut cbuf, id, b"ctl");
    let Some(ctl) = open_flags(&cbuf[..cn], O_WRONLY) else {
        fail(b"open ctl fail\n");
    };
    let mut ctl_cmd = [0u8; 40];
    let cm = connect_ctl(ip_bytes, port, &mut ctl_cmd);
    if write_fd(ctl, &ctl_cmd[..cm]) == usize::MAX {
        close(ctl);
        fail(b"write ctl fail\n");
    }
    close(ctl);

    // ------------------------------------------------------------------
    // Step 3: wait for handshake — status says "connected"
    // ------------------------------------------------------------------
    let mut sbuf = [0u8; 64];
    let sn = conv_path(&mut cbuf, id, b"status");
    let mut connected = false;
    for _ in 0..STATUS_POLLS {
        let Some(st) = open(&cbuf[..sn]) else {
            // Briefly yield so the daemon isn't starved; keep polling.
            continue;
        };
        let nr = read(st, &mut sbuf);
        close(st);
        if nr > 0 && nr <= 64 && buf_has(&sbuf[..nr], b"connected") {
            connected = true;
            break;
        }
        // If read returns 0 that just means the channel was momentarily empty;
        // keep polling.  If it returns usize::MAX that's an error — break.
        if nr == usize::MAX {
            break;
        }
    }
    if !connected {
        fail(b"tcp connect timeout\n");
    }

    // ------------------------------------------------------------------
    // Step 4: data — send the HTTP GET request
    // ------------------------------------------------------------------
    let mut dbuf = [0u8; 40];
    let dn = conv_path(&mut dbuf, id, b"data");
    let Some(data) = open_flags(&dbuf[..dn], O_RDWR) else {
        fail(b"open data fail\n");
    };

    // Build: "GET /path HTTP/1.1\r\nHost: ip\r\nConnection: close\r\n\r\n"
    let need = 4 + ip_bytes.len() + 15 + path.len() + 19;
    let mut req = [0u8; 256];
    if need > req.len() {
        close(data);
        fail(b"request too long\n");
    }
    let mut q = 0usize;
    req[q..q + 4].copy_from_slice(b"GET ");
    q += 4;
    req[q..q + path.len()].copy_from_slice(path);
    q += path.len();
    req[q..q + 15].copy_from_slice(b" HTTP/1.1\r\nHost: ");
    q += 15;
    req[q..q + ip_bytes.len()].copy_from_slice(ip_bytes);
    q += ip_bytes.len();
    req[q..q + 19].copy_from_slice(b"\r\nConnection: close\r\n\r\n");
    q += 19;
    if write_fd(data, &req[..q]) == usize::MAX {
        close(data);
        fail(b"write data fail\n");
    }

    // ------------------------------------------------------------------
    // Step 5: read the response until peer closes or we run out of polls.
    // NOTE: netfs read() returns 0 when the buffer is momentarily empty
    // (not EOF).  We loop on 0 and only bail on error (usize::MAX) or budget exhaustion.
    // ------------------------------------------------------------------
    let mut got = false;
    let mut total = 0usize;
    let mut rbuf = [0u8; BUF];
    for _ in 0..DATA_POLLS {
        let nr = read(data, &mut rbuf);
        if nr == usize::MAX {
            break; // kernel error
        }
        if nr == 0 {
            continue; // momentarily empty; keep waiting
        }
        got = true;
        total += nr;
        write(&rbuf[..nr]);
    }
    close(data);
    if !got {
        fail(b"http no data\n");
    }
    write(b"\n");
    write(b"http ok\n");
    // (total is just for diagnostics; not printed to avoid extra syscalls)
    exit();
}

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    myos_user::panic_die(info);
}