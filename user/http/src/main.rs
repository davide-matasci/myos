#![no_std]
#![no_main]

extern crate alloc;

use myos_tls::TlsConn;
use myos_user::dns::{format_ipv4, resolve_a};
use myos_user::{status_ok, 
    close, exit, heap_init, open, open_flags, read, write, write_fd, Heap, O_RDWR, O_WRONLY,
};

#[global_allocator]
static GLOBAL: Heap = Heap;

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
    write(b"usage: http <url|ipv4> [port] [path]\n");
    write(b"  https://host[/path]  http://host[/path]  host  ipv4\n");
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

struct Target {
    https: bool,
    host: [u8; 128],
    host_len: usize,
    ip: [u8; 16],
    ip_len: usize,
    port: u16,
    path: [u8; 128],
    path_len: usize,
}

fn copy_to(dst: &mut [u8], src: &[u8]) -> usize {
    let n = src.len().min(dst.len());
    dst[..n].copy_from_slice(&src[..n]);
    n
}

fn parse_port_num(s: &[u8]) -> Option<u16> {
    if s.is_empty() || s.len() > 5 {
        return None;
    }
    let mut n = 0u32;
    for &b in s {
        if !b.is_ascii_digit() {
            return None;
        }
        n = n.checked_mul(10)?.checked_add((b - b'0') as u32)?;
    }
    if n == 0 || n > 65535 {
        None
    } else {
        Some(n as u16)
    }
}

fn parse_target(arg: &[u8], port_arg: Option<&[u8]>, path_arg: Option<&[u8]>) -> Target {
    let mut t = Target {
        https: false,
        host: [0; 128],
        host_len: 0,
        ip: [0; 16],
        ip_len: 0,
        port: 80,
        path: [0; 128],
        path_len: 0,
    };

    let mut rest = arg;
    if rest.starts_with(b"https://") {
        t.https = true;
        t.port = 443;
        rest = &rest[8..];
    } else if rest.starts_with(b"http://") {
        t.https = false;
        t.port = 80;
        rest = &rest[7..];
    }

    // split host[:port][/path]
    let mut hostpart = rest;
    let mut pathpart: &[u8] = b"/";
    if let Some(i) = rest.iter().position(|&b| b == b'/') {
        hostpart = &rest[..i];
        pathpart = &rest[i..];
    }

    let mut host_only = hostpart;
    if let Some(i) = hostpart.iter().position(|&b| b == b':') {
        host_only = &hostpart[..i];
        if let Some(p) = parse_port_num(&hostpart[i + 1..]) {
            t.port = p;
            if p == 443 {
                t.https = true;
            }
        } else {
            fail(b"bad port in url\n");
        }
    }

    if host_only.is_empty() {
        fail(b"missing host\n");
    }

    // Legacy: http <ipv4> [port] [path]
    if parse_ipv4(arg) {
        t.https = false;
        t.port = 80;
        t.host_len = copy_to(&mut t.host, arg);
        t.ip_len = copy_to(&mut t.ip, arg);
        if let Some(p) = port_arg {
            t.port = parse_port_num(p).unwrap_or_else(|| fail(b"bad port\n"));
            if t.port == 443 {
                t.https = true;
            }
        }
        if let Some(p) = path_arg {
            if p.is_empty() || p[0] != b'/' {
                fail(b"path must start with /\n");
            }
            t.path_len = copy_to(&mut t.path, p);
        } else {
            t.path_len = copy_to(&mut t.path, b"/");
        }
        return t;
    }

    if let Some(p) = port_arg {
        // If first arg was URL, extra args are unusual; allow override.
        t.port = parse_port_num(p).unwrap_or_else(|| fail(b"bad port\n"));
    }
    if let Some(p) = path_arg {
        if p.is_empty() || p[0] != b'/' {
            fail(b"path must start with /\n");
        }
        pathpart = p;
    }

    t.host_len = copy_to(&mut t.host, host_only);
    t.path_len = copy_to(&mut t.path, pathpart);
    if t.path_len == 0 {
        t.path_len = copy_to(&mut t.path, b"/");
    }

    if parse_ipv4(host_only) {
        t.ip_len = copy_to(&mut t.ip, host_only);
    } else {
        match resolve_a(host_only) {
            Ok(ip) => {
                t.ip_len = format_ipv4(ip, &mut t.ip);
            }
            Err(_) => fail(b"dns resolve fail\n"),
        }
    }
    t
}

fn tcp_connect(ip: &[u8], port: u16) -> (u16, usize) {
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
                id = match id.checked_mul(10).and_then(|x| x.checked_add((b - b'0') as u16)) {
                    Some(y) => y,
                    None => fail(b"clone id fail\n"),
                };
            }
        }
    }
    if !any {
        fail(b"clone id fail\n");
    }

    let mut pbuf = [0u8; 40];
    let pn = conv_path(&mut pbuf, id, b"ctl");
    let Some(ctl) = open_flags(&pbuf[..pn], O_WRONLY) else {
        fail(b"open ctl fail\n");
    };
    let mut cmd = [0u8; 40];
    let cm = connect_ctl(ip, port, &mut cmd);
    if write_fd(ctl, &cmd[..cm]) == usize::MAX {
        close(ctl);
        fail(b"write ctl fail\n");
    }
    close(ctl);

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

    let dn = conv_path(&mut pbuf, id, b"data");
    let Some(data) = open_flags(&pbuf[..dn], O_RDWR) else {
        fail(b"open data fail\n");
    };
    (id, data)
}

fn build_request(t: &Target, out: &mut [u8]) -> usize {
    let mut q = 0usize;
    let put = |out: &mut [u8], q: &mut usize, s: &[u8]| {
        if *q + s.len() > out.len() {
            fail(b"request too long\n");
        }
        out[*q..*q + s.len()].copy_from_slice(s);
        *q += s.len();
    };
    put(out, &mut q, b"GET ");
    put(out, &mut q, &t.path[..t.path_len]);
    put(out, &mut q, b" HTTP/1.1\r\nHost: ");
    put(out, &mut q, &t.host[..t.host_len]);
    put(out, &mut q, b"\r\nConnection: close\r\n\r\n");
    q
}

fn main() -> ! {
    heap_init();

    let Some(arg1) = myos_user::arg(1) else {
        usage();
    };
    let t = parse_target(arg1, myos_user::arg(2), myos_user::arg(3));

    let (_id, data) = tcp_connect(&t.ip[..t.ip_len], t.port);
    let mut req = [0u8; 512];
    let q = build_request(&t, &mut req);

    if t.https {
        let mut sni = [0u8; 129];
        if t.host_len >= sni.len() {
            close(data);
            fail(b"host too long\n");
        }
        sni[..t.host_len].copy_from_slice(&t.host[..t.host_len]);
        // NUL already present

        let mut tls = TlsConn::new();
        if let Err(e) = tls.handshake(data, &sni[..t.host_len + 1]) {
            close(data);
            write(b"tls err ");
            let mut v = e;
            if v < 0 {
                write(b"-");
                v = -v;
            }
            let mut digs = [0u8; 12];
            let mut n = 0usize;
            if v == 0 {
                digs[0] = b'0';
                n = 1;
            }
            while v > 0 && n < digs.len() {
                digs[n] = b'0' + ((v % 10) as u8);
                v /= 10;
                n += 1;
            }
            while n > 0 {
                n -= 1;
                let b = [digs[n]];
                write(&b);
            }
            write(b"\n");
            fail(b"tls handshake fail\n");
        }
        if tls.write_all(&req[..q]).is_err() {
            tls.close();
            close(data);
            fail(b"tls write fail\n");
        }

        let mut got = false;
        let mut empty_polls = 0usize;
        let mut rbuf = [0u8; BUF];
        for _ in 0..DATA_POLLS {
            match tls.read(&mut rbuf) {
                Ok(0) => {
                    if got {
                        empty_polls += 1;
                        if empty_polls > 10000 {
                            break;
                        }
                    }
                }
                Ok(n) => {
                    got = true;
                    empty_polls = 0;
                    write(&rbuf[..n]);
                }
                Err(_) => break,
            }
        }
        tls.close();
        close(data);
        if !got {
            fail(b"https no data\n");
        }
        write(b"\n");
        status_ok("https");
        exit();
    }

    // Plain HTTP
    if write_fd(data, &req[..q]) == usize::MAX {
        close(data);
        fail(b"write request fail\n");
    }
    let mut got = false;
    let mut empty_polls = 0usize;
    let mut rbuf = [0u8; BUF];
    for _ in 0..DATA_POLLS {
        let nr = read(data, &mut rbuf);
        if nr == usize::MAX {
            break;
        }
        if nr == 0 {
            if got {
                empty_polls += 1;
                if empty_polls > 10000 {
                    break;
                }
            }
            continue;
        }
        got = true;
        write(&rbuf[..nr]);
    }
    close(data);
    if !got {
        fail(b"http no data\n");
    }
    write(b"\n");
    status_ok("http");
    exit();
}

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    myos_user::panic_die(info);
}
