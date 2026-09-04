#![no_std]
#![no_main]

use myos_user::{close, exit, open, open_flags, read, write, write_fd, O_RDWR, O_WRONLY};

myos_user::x86_start!(main);

#[cfg(any(target_arch = "aarch64", target_arch = "riscv64"))]
#[unsafe(no_mangle)]
pub extern "C" fn _start(argc: usize, argv: *const usize) -> ! {
    unsafe { myos_user::args::init_from_regs(argc, argv) };
    main()
}

const STATUS_POLLS: usize = 200_000;
const DATA_POLLS: usize = 400_000;
const BUF: usize = 1024;

fn usage() -> ! {
    write(b"usage: http <ipv4> [port] [path]\n");
    write(b"  default port 80, default path /\n");
    exit();
}

fn fail(msg: &[u8]) -> ! {
    write(msg);
    exit();
}

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
            if i >= s.len() || s[i] != b'.' {
                return false;
            }
            i += 1;
        }
    }
    parts == 4 && i == s.len()
}

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

fn buf_has(hay: &[u8], needle: &[u8]) -> bool {
    hay.windows(needle.len()).any(|w| w == needle)
}

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
    let ip_bytes = ip_str;
    if !parse_ipv4(ip_bytes) {
        fail(b"bad ipv4\n");
    }
    let port: u16 = match myos_user::arg(2) {
        Some(p) => {
            if !parse_port(p) {
                fail(b"bad port\n");
            }
            let mut n = 0u32;
            for &b in p {
                n = n * 10 + (b - b'0') as u32;
            }
            n as u16
        }
        None => 80,
    };
    let path: &[u8] = match myos_user::arg(3) {
        Some(p) => {
            if p.is_empty() || p[0] != b'/' {
                fail(b"path must start with /\n");
            }
            p
        }
        None => b"/",
    };

    // Step 1: clone a TCP conversation
    let Some(clone) = open(b"/net/tcp/clone") else {
        fail(b"open /net/tcp/clone fail\n");
    };
    let mut idbuf = [0u8; 16];
    let n = read(clone, &mut idbuf);
    close(clone);
    if n == 0 || n == usize::MAX {
        fail(b"read clone fail\n");
    }
    let mut id: u16 = 0;
    let mut any = false;
    for &b in &idbuf[..n] {
        match b {
            b'\n' | b'\r' | b' ' => break,
            _ => {
                if !b.is_ascii_digit() {
                    fail(b"clone id fail\n");
                }
                any = true;
                id = match id.checked_mul(10) {
                    Some(x) => match x.checked_add((b - b'0') as u16) {
                        Some(y) => y,
                        None => { fail(b"clone id fail\n"); }
                    },
                    None => { fail(b"clone id fail\n"); }
                };
            }
        }
    }
    if !any {
        fail(b"clone id fail\n");
    }

    // Step 2: send connect ip!port
    let mut pbuf = [0u8; 40];
    let pn = conv_path(&mut pbuf, id, b"ctl");
    let Some(ctl) = open_flags(&pbuf[..pn], O_WRONLY) else {
        fail(b"open ctl fail\n");
    };
    let mut cmd = [0u8; 40];
    let cm = connect_ctl(ip_bytes, port, &mut cmd);
    if write_fd(ctl, &cmd[..cm]) == usize::MAX {
        close(ctl);
        fail(b"write ctl fail\n");
    }
    close(ctl);

    // Step 3: wait for connected status
    let sn = conv_path(&mut pbuf, id, b"status");
    let mut connected = false;
    for _ in 0..STATUS_POLLS {
        let Some(st) = open(&pbuf[..sn]) else {
            continue;
        };
        let mut sbuf = [0u8; 64];
        let nr = read(st, &mut sbuf);
        close(st);
        if nr > 0 && nr <= 64 && buf_has(&sbuf[..nr], b"connected") {
            connected = true;
            break;
        }
    }
    if !connected {
        fail(b"tcp connect timeout\n");
    }

    // Step 4: open data, send HTTP request
    let dn = conv_path(&mut pbuf, id, b"data");
    let Some(data) = open_flags(&pbuf[..dn], O_RDWR) else {
        fail(b"open data fail\n");
    };

    // Build: GET <path> HTTP/1.1\r\nHost: <ip>\r\nConnection: close\r\n\r\n
    let mut req = [0u8; 256];
    let mut q = 0usize;

    req[q] = b'G'; req[q+1] = b'E'; req[q+2] = b'T'; req[q+3] = b' ';
    q += 4;

    if q + path.len() > 256 { close(data); fail(b"path too long\n"); }
    req[q..q + path.len()].copy_from_slice(path);
    q += path.len();

    let http_line = b" HTTP/1.1\r\n";
    if q + http_line.len() > 256 { close(data); fail(b"path too long\n"); }
    req[q..q + http_line.len()].copy_from_slice(http_line);
    q += http_line.len();

    let host_line = b"Host: ";
    if q + host_line.len() + ip_bytes.len() + 2 > 256 { close(data); fail(b"host too long\n"); }
    req[q..q + host_line.len()].copy_from_slice(host_line);
    q += host_line.len();

    req[q..q + ip_bytes.len()].copy_from_slice(ip_bytes);
    q += ip_bytes.len();

    req[q] = b'\r'; req[q+1] = b'\n';
    q += 2;

    let end_headers = b"Connection: close\r\n\r\n";
    if q + end_headers.len() > 256 { close(data); fail(b"headers too long\n"); }
    req[q..q + end_headers.len()].copy_from_slice(end_headers);
    q += end_headers.len();

    if write_fd(data, &req[..q]) == usize::MAX {
        close(data);
        fail(b"write request fail\n");
    }

    // Step 5: read response
    let mut got = false;
    let mut empty_polls = 0usize;
    let mut rbuf = [0u8; BUF];
    for _ in 0..DATA_POLLS {
        let nr = read(data, &mut rbuf);
        if nr == usize::MAX {
            break; // kernel error
        }
        if nr == 0 {
            if got {
                empty_polls += 1;
                if empty_polls > 10000 {
                    break; // long idle after data = peer closed
                }
            }
            continue; // momentarily empty; keep waiting
        }
        got = true;
        write(&rbuf[..nr]);
    }
    close(data);
    if !got {
        fail(b"http no data\n");
    }
    write(b"\n");
    write(b"http ok\n");
    exit();
}

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    myos_user::panic_die(info);
}