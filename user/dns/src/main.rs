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
    write(b"usage: dns <hostname>\n");
    exit();
}

fn fail(msg: &[u8]) -> ! {
    write(msg);
    exit();
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
    let prefix = b"/net/udp/";
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

fn encode_name(dst: &mut [u8], name: &[u8]) -> usize {
    let mut pos = 0usize;
    let mut start = 0usize;
    while start < name.len() {
        let mut end = start;
        while end < name.len() && name[end] != b'.' {
            end += 1;
        }
        let len = end - start;
        if len > 63 {
            return 0;
        }
        dst[pos] = len as u8;
        pos += 1;
        dst[pos..pos + len].copy_from_slice(&name[start..end]);
        pos += len;
        if end == name.len() {
            break;
        }
        start = end + 1;
    }
    dst[pos] = 0;
    pos + 1
}

fn parse_ipv4(buf: &[u8]) -> Option<[u8; 4]> {
    for window in buf.windows(4) {
        if window.len() == 4 {
            return Some([window[0], window[1], window[2], window[3]]);
        }
    }
    None
}

fn main() -> ! {
    let Some(host) = myos_user::arg(1) else {
        usage();
    };

    // DNS server: 10.0.2.3 (QEMU default DNS proxy) or 8.8.8.8
    let dns_server = b"10.0.2.3";
    let dns_port: u16 = 53;

    // Build DNS query
    let mut query = [0u8; 512];
    let mut qpos = 0usize;

    // Transaction ID
    query[qpos] = 0x12;
    query[qpos + 1] = 0x34;
    qpos += 2;

    // Flags: standard query, recursion desired
    query[qpos] = 0x01;
    query[qpos + 1] = 0x00;
    qpos += 2;

    // QDCOUNT = 1
    query[qpos] = 0x00;
    query[qpos + 1] = 0x01;
    qpos += 2;

    // ANCOUNT, NSCOUNT, ARCOUNT = 0
    qpos += 6;

    // Question: name
    let name_len = encode_name(&mut query[qpos..], host);
    if name_len == 0 {
        fail(b"name too long\n");
    }
    qpos += name_len;

    // QTYPE = A (1)
    query[qpos] = 0x00;
    query[qpos + 1] = 0x01;
    qpos += 2;

    // QCLASS = IN (1)
    query[qpos] = 0x00;
    query[qpos + 1] = 0x01;
    qpos += 2;

    // Step 1: clone UDP conversation
    let Some(clone) = open(b"/net/udp/clone") else {
        fail(b"open /net/udp/clone fail\n");
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

    // Step 2: send connect
    let mut pbuf = [0u8; 40];
    let pn = conv_path(&mut pbuf, id, b"ctl");
    let Some(ctl) = open_flags(&pbuf[..pn], O_WRONLY) else {
        fail(b"open ctl fail\n");
    };
    let mut cmd = [0u8; 40];
    let cm = connect_ctl(dns_server, dns_port, &mut cmd);
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
        fail(b"udp connect timeout\n");
    }

    // Step 4: send DNS query
    let dn = conv_path(&mut pbuf, id, b"data");
    let Some(data) = open_flags(&pbuf[..dn], O_RDWR) else {
        fail(b"open data fail\n");
    };

    if write_fd(data, &query[..qpos]) == usize::MAX {
        close(data);
        fail(b"write query fail\n");
    }

    // Step 5: read DNS response
    let mut got = false;
    let mut rbuf = [0u8; BUF];
    for _ in 0..DATA_POLLS {
        let nr = read(data, &mut rbuf);
        if nr == usize::MAX {
            break;
        }
        if nr == 0 {
            continue;
        }
        got = true;
        // Parse DNS response for A records
        if let Some(ip) = parse_ipv4(&rbuf[..nr]) {
            write(b"IP: ");
            let mut ip_str = [0u8; 16];
            let mut pos = 0usize;
            for (i, octet) in ip.iter().enumerate() {
                let mut tmp = [0u8; 4];
                let mut n = *octet as u16;
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
                if pos + digits.len() + 1 > 16 {
                    break;
                }
                ip_str[pos..pos + digits.len()].copy_from_slice(digits);
                pos += digits.len();
                if i < 3 {
                    ip_str[pos] = b'.';
                    pos += 1;
                }
            }
            write(&ip_str[..pos]);
            write(b"\n");
            break;
        }
    }
    close(data);
    if !got {
        fail(b"dns no response\n");
    }
    write(b"dns ok\n");
    exit();
}

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    myos_user::panic_die(info);
}