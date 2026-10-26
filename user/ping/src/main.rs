#![no_std]
#![deny(unsafe_op_in_unsafe_fn)]
#![no_main]

use myos_user::{status_ok, close, exit, exit_code, open, open_flags, read, write, write_fd, O_WRONLY};

myos_user::x86_start!(main);

#[cfg(any(target_arch = "aarch64", target_arch = "riscv64"))]
#[unsafe(no_mangle)]
pub extern "C" fn _start(argc: usize, argv: *const usize) -> ! {
    unsafe { myos_user::args::init_from_regs(argc, argv) };
    main()
}

/// Wall-clock budgets (iteration counts scaled with syscall speed).
const STATUS_TIMEOUT_MS: i64 = 5_000;
const DATA_TIMEOUT_MS: i64 = 5_000;

fn usage() -> ! {
    write(b"usage: ping <ipv4|ipv6>\n");
    exit_code(1);
}

fn fail(msg: &[u8]) -> ! {
    write(msg);
    exit_code(1);
}

/// An IPv4 address `a.b.c.d` or an IPv6 one (netd echoes over ICMPv6).
fn parse_ip(s: &[u8]) -> bool {
    core::str::from_utf8(s).ok().is_some_and(|t| t.parse::<core::net::IpAddr>().is_ok())
}

fn connect_ctl(ip: &[u8], out: &mut [u8]) -> usize {
    let prefix = b"connect ";
    let n = prefix.len() + ip.len();
    out[..prefix.len()].copy_from_slice(prefix);
    out[prefix.len()..n].copy_from_slice(ip);
    n
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
    let Some(ip) = myos_user::arg(1) else {
        usage();
    };
    if !parse_ip(ip) {
        usage();
    }

    // The fd is on the new conversation's data: its last close (exit
    // included) hangs it up.
    let Some((data, id)) = myos_user::clone_conv(b"/net/icmp") else {
        fail(b"open /net/icmp/clone fail\n");
    };

    let mut path = [0u8; 40];
    let pn = conv_path(&mut path, id, b"ctl");
    let Some(ctl) = open_flags(&path[..pn], O_WRONLY) else {
        fail(b"open ctl fail\n");
    };
    let mut ctl_cmd = [0u8; 24];
    let cn = connect_ctl(ip, &mut ctl_cmd);
    if write_fd(ctl, &ctl_cmd[..cn]) == usize::MAX {
        close(ctl);
        fail(b"write ctl fail\n");
    }
    close(ctl);

    let sn = conv_path(&mut path, id, b"status");
    let mut connected = false;
    let mut t = myos_user::Timeout::ms(STATUS_TIMEOUT_MS);
    while !t.expired() {
        if let Some(st) = open(&path[..sn]) {
            let mut sbuf = [0u8; 64];
            let nr = read(st, &mut sbuf);
            close(st);
            if nr != 0 && nr != usize::MAX && buf_has(&sbuf[..nr], b"connected") {
                connected = true;
                break;
            }
        }
        t.wait();
    }
    if !connected {
        fail(b"icmp status timeout\n");
    }

    if write_fd(data, b"ping") == usize::MAX {
        close(data);
        fail(b"write data fail\n");
    }
    let mut got = false;
    let mut t = myos_user::Timeout::ms(DATA_TIMEOUT_MS);
    while !t.expired() {
        let mut rbuf = [0u8; 64];
        let nr = read(data, &mut rbuf);
        if nr != 0 && nr != usize::MAX {
            got = true;
            break;
        }
        if nr == usize::MAX {
            break;
        }
        t.wait();
    }
    close(data);
    if !got {
        fail(b"icmp data timeout\n");
    }
    status_ok("ping");
    exit();
}

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    myos_user::panic_die(info);
}