//! `wifi`: the Wi-Fi daemon and its command (issue #495, `docs/wifi.md`).
//!
//! - `wifi daemon [NIC]` drives the radio: the simulated one on NIC
//!   (default `/dev/net1`, the test boots' second NIC, `src/wifi_sim.rs`).
//!   It listens on the unix socket `wifid` for the commands below, one per
//!   connection: a line in, the answer out, then the connection ends.
//! - `wifi scan`: a scan of the 2.4 GHz channels; prints what answered.
//! - `wifi list`: what the last scan found.
//!
//! A network is one line: `BSSID CHANNEL RSSI SECURITY SSID`, the strongest
//! first (`02:00:5e:00:01:01 1 -40 open myos-open`); bytes of the SSID
//! outside printable ASCII are written `\xHH`.

#![no_std]
#![deny(unsafe_op_in_unsafe_fn)]
#![no_main]

extern crate alloc;

mod frame;
mod radio;
mod unix;

use alloc::vec::Vec;

use frame::Bss;
use myos_user::{exit_code, heap_init, now_us, poll, write, Heap, PollFd, POLLIN};
use radio::{Radio, Sim};
use unix::Conv;

#[global_allocator]
static GLOBAL: Heap = Heap;

myos_user::x86_start!(main);

#[cfg(any(target_arch = "aarch64", target_arch = "riscv64"))]
#[unsafe(no_mangle)]
pub extern "C" fn _start(argc: usize, argv: *const usize) -> ! {
    unsafe { myos_user::args::init_from_regs(argc, argv) };
    main()
}

/// The 2.4 GHz channels a scan visits.
const CHANNELS: core::ops::RangeInclusive<u8> = 1..=13;
/// How long a scan listens on each channel after its probe request, in
/// ms: beacons come every 100 TU (102.4 ms), so one is heard too.
const DWELL_MS: i64 = 120;

fn usage() -> ! {
    write(b"usage: wifi daemon [NIC] | wifi scan | wifi list\n");
    exit_code(2);
}

fn main() -> ! {
    heap_init();
    match myos_user::args::arg(1) {
        Some(b"daemon") => daemon(myos_user::args::arg(2).unwrap_or(b"/dev/net1")),
        Some(cmd @ (b"scan" | b"list")) => client(cmd),
        _ => usage(),
    }
}

/// A network a scan heard.
struct Seen {
    bss: Bss,
    rssi: i8,
}

fn daemon(nic: &[u8]) -> ! {
    let Some(mut radio) = Sim::open(nic) else {
        write(b"wifi: no radio\n");
        exit_code(1);
    };
    let mut cmd = Vec::from(b"announce " as &[u8]);
    cmd.extend_from_slice(unix::NAME);
    let Some(listener) = Conv::new(&cmd) else {
        write(b"wifi: the socket wifid is taken\n");
        exit_code(1);
    };
    let mut seen: Vec<Seen> = Vec::new();
    loop {
        let mut fds = [PollFd { fd: listener.data as i32, events: POLLIN, revents: 0 }];
        poll(&mut fds, -1);
        while let Some(conn) = listener.accept() {
            let mut buf = [0u8; 64];
            let n = conn.read(&mut buf, 2000);
            match buf[..n].trim_ascii() {
                b"scan" => {
                    seen = scan(&mut radio);
                    conn.write(&listing(&seen));
                }
                b"list" => {
                    conn.write(&listing(&seen));
                }
                _ => {
                    conn.write(b"error: unknown command\n");
                }
            }
            conn.close();
        }
    }
}

/// Visit every channel: a wildcard probe request, then whatever beacons and
/// probe responses are heard for [`DWELL_MS`].
fn scan(radio: &mut dyn Radio) -> Vec<Seen> {
    let mut seen: Vec<Seen> = Vec::new();
    let mut buf = [0u8; 2048];
    for ch in CHANNELS {
        radio.set_channel(ch);
        radio.send(&frame::probe_request(radio.mac()));
        let end = now_us().unwrap_or(0) + DWELL_MS * 1000;
        loop {
            while let Some(rx) = radio.recv(&mut buf) {
                let Some(bss) = frame::parse_bss(&buf[..rx.len], rx.channel) else {
                    continue;
                };
                match seen.iter_mut().find(|s| s.bss.bssid == bss.bssid) {
                    Some(s) => s.rssi = s.rssi.max(rx.rssi),
                    None => seen.push(Seen { bss, rssi: rx.rssi }),
                }
            }
            let left = end - now_us().unwrap_or(end);
            if left <= 0 {
                break;
            }
            let mut fds = [PollFd { fd: radio.fd() as i32, events: POLLIN, revents: 0 }];
            poll(&mut fds, (left / 1000).max(1) as i32);
        }
    }
    seen.sort_by(|a, b| b.rssi.cmp(&a.rssi));
    seen
}

fn listing(seen: &[Seen]) -> Vec<u8> {
    let mut out = Vec::new();
    for s in seen {
        for (i, b) in s.bss.bssid.iter().enumerate() {
            if i != 0 {
                out.push(b':');
            }
            hex(&mut out, *b);
        }
        out.push(b' ');
        dec(&mut out, i64::from(s.bss.channel));
        out.push(b' ');
        dec(&mut out, i64::from(s.rssi));
        out.push(b' ');
        out.extend_from_slice(s.bss.security.name());
        out.push(b' ');
        for &c in &s.bss.ssid {
            if (0x21..0x7f).contains(&c) && c != b'\\' {
                out.push(c);
            } else {
                out.extend_from_slice(b"\\x");
                hex(&mut out, c);
            }
        }
        out.push(b'\n');
    }
    out
}

fn hex(out: &mut Vec<u8>, b: u8) {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    out.push(DIGITS[(b >> 4) as usize]);
    out.push(DIGITS[(b & 15) as usize]);
}

fn dec(out: &mut Vec<u8>, v: i64) {
    if v < 0 {
        out.push(b'-');
    }
    let mut d = [0u8; 20];
    let mut i = d.len();
    let mut v = v.unsigned_abs();
    loop {
        i -= 1;
        d[i] = b'0' + (v % 10) as u8;
        v /= 10;
        if v == 0 {
            break;
        }
    }
    out.extend_from_slice(&d[i..]);
}

/// Ask the daemon: the command out, its answer to stdout until it ends the
/// connection.
fn client(cmd: &[u8]) -> ! {
    let mut connect = Vec::from(b"connect " as &[u8]);
    connect.extend_from_slice(unix::NAME);
    let Some(conn) = Conv::new(&connect) else {
        write(b"wifi: no daemon (wifi daemon)\n");
        exit_code(1);
    };
    let mut line = Vec::from(cmd);
    line.push(b'\n');
    if !conn.write(&line) {
        exit_code(1);
    }
    let mut buf = [0u8; 1024];
    let mut error = false;
    loop {
        // A scan takes CHANNELS × DWELL_MS: well within the wait.
        let n = conn.read(&mut buf, 30_000);
        if n == 0 {
            break;
        }
        error |= buf[..n].starts_with(b"error:");
        write(&buf[..n]);
    }
    conn.close();
    exit_code(if error { 1 } else { 0 });
}

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    myos_user::panic_die(info);
}
