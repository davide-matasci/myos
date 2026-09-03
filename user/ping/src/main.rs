#![no_std]
#![no_main]

use myos_user::{close, exit, exit_code, open, open_flags, read, write, write_fd, O_RDWR, O_WRONLY};

const GATEWAY_CTL: &[u8] = b"connect 10.0.2.2";
const STATUS_POLLS: usize = 8000;
const DATA_POLLS: usize = 8000;

#[cfg(target_arch = "x86_64")]
#[unsafe(no_mangle)]
pub extern "C" fn _start() -> ! {
    main()
}

#[cfg(any(target_arch = "aarch64", target_arch = "riscv64"))]
#[unsafe(no_mangle)]
pub extern "C" fn _start(_argc: usize, _argv: *const usize) -> ! {
    main()
}

fn fail(msg: &[u8]) -> ! {
    write(msg);
    exit_code(1);
}

fn parse_id(buf: &[u8]) -> Option<u16> {
    let mut n = 0u32;
    let mut any = false;
    for &b in buf {
        if b == b'\n' || b == b'\r' || b == b' ' {
            break;
        }
        if !b.is_ascii_digit() {
            return None;
        }
        any = true;
        n = n.checked_mul(10)?.checked_add((b - b'0') as u32)?;
        if n > 65535 {
            return None;
        }
    }
    if any { Some(n as u16) } else { None }
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
    let mut n = 0usize;
    let prefix = b"/net/icmp/";
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

fn main() -> ! {
    let Some(clone) = open(b"/net/icmp/clone") else {
        fail(b"open /net/icmp/clone fail\n");
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

    let mut path = [0u8; 40];
    let pn = conv_path(&mut path, id, b"ctl");
    let Some(ctl) = open_flags(&path[..pn], O_WRONLY) else {
        fail(b"open ctl fail\n");
    };
    if write_fd(ctl, GATEWAY_CTL) == usize::MAX {
        close(ctl);
        fail(b"write ctl fail\n");
    }
    close(ctl);

    let sn = conv_path(&mut path, id, b"status");
    let mut connected = false;
    for _ in 0..STATUS_POLLS {
        let Some(st) = open(&path[..sn]) else {
            continue;
        };
        let mut sbuf = [0u8; 64];
        let nr = read(st, &mut sbuf);
        close(st);
        if nr != 0 && nr != usize::MAX && buf_has(&sbuf[..nr], b"connected") {
            connected = true;
            break;
        }
    }
    if !connected {
        fail(b"icmp status timeout\n");
    }

    let dn = conv_path(&mut path, id, b"data");
    let Some(data) = open_flags(&path[..dn], O_RDWR) else {
        fail(b"open data fail\n");
    };
    if write_fd(data, b"ping") == usize::MAX {
        close(data);
        fail(b"write data fail\n");
    }
    let mut got = false;
    for _ in 0..DATA_POLLS {
        let mut rbuf = [0u8; 64];
        let nr = read(data, &mut rbuf);
        if nr != 0 && nr != usize::MAX {
            got = true;
            break;
        }
    }
    close(data);
    if !got {
        fail(b"icmp data timeout\n");
    }
    write(b"ping ok\n");
    exit();
}

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    myos_user::panic_die(info);
}
