//! `wifi`: the Wi-Fi daemon and its command (issue #495, `docs/wifi.md`).
//!
//! - `wifi daemon [NIC]` drives the radio: the simulated one on NIC
//!   (default `/dev/net1`, the test boots' second NIC, `src/wifi_sim.rs`).
//!   It listens on the unix socket `wifid` for the commands below, one per
//!   connection: a line in, the answer out, then the connection ends.
//! - `wifi scan`: a scan of the 2.4 GHz channels; prints what answered.
//! - `wifi list`: what the last scan found.
//! - `wifi connect SSID`: join the network SSID (the strongest the last
//!   scan heard, after a scan when it heard none): open networks only for
//!   now, WPA2 is next (issue #495).
//! - `wifi status`: `associated BSSID CHANNEL SSID aid N`, or
//!   `disconnected`.
//! - `wifi disconnect`: leave the network.
//!
//! While associated, the link is a virtual NIC of `/dev/tap` for netd
//! (`tapN` in `/net/ifaddrs`, its address from the network's DHCP): the
//! daemon carries its Ethernet frames as 802.11 data frames.
//!
//! A network is one line: `BSSID CHANNEL RSSI SECURITY SSID`, the strongest
//! first (`02:00:5e:00:01:01 1 -40 open myos-open`); bytes of the SSID
//! outside printable ASCII are written `\xHH`.

#![no_std]
#![deny(unsafe_op_in_unsafe_fn)]
#![no_main]

extern crate alloc;

mod frame;
mod mlme;
mod radio;
mod tap;
mod unix;

use alloc::vec::Vec;

use frame::{Bss, Security};
use mlme::{Link, Refused};
use myos_user::{exit_code, heap_init, monotonic_ns, poll, write, Heap, PollFd, POLLIN};
use radio::{Radio, Sim};
use tap::Tap;
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
const DWELL_MS: u64 = 120;

fn usage() -> ! {
    write(b"usage: wifi daemon [NIC] | wifi scan | list | connect SSID | status | disconnect\n");
    exit_code(2);
}

fn main() -> ! {
    heap_init();
    match myos_user::args::arg(1) {
        Some(b"daemon") => daemon(myos_user::args::arg(2).unwrap_or(b"/dev/net1")),
        Some(b"scan" | b"list" | b"connect" | b"status" | b"disconnect") => client(),
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
    let mut link: Option<Link> = None;
    // The link's NIC, there while `link` is.
    let mut tap: Option<Tap> = None;
    let mut frame = [0u8; 2048];
    loop {
        let mut fds = Vec::from([
            PollFd { fd: listener.data as i32, events: POLLIN, revents: 0 },
            PollFd { fd: radio.fd() as i32, events: POLLIN, revents: 0 },
        ]);
        if let Some(t) = &tap {
            fds.push(PollFd { fd: t.fd() as i32, events: POLLIN, revents: 0 });
        }
        poll(&mut fds, -1);
        // What the radio hears between commands: data for the link's NIC;
        // the access point letting the station go ends the link.
        while let Some(rx) = radio.recv(&mut frame) {
            let Some(l) = link.as_ref() else {
                continue;
            };
            let f = &frame[..rx.len];
            if let Some(eth) = frame::data_from_ds(f, radio.mac(), l.bss.bssid) {
                if let Some(t) = &tap {
                    t.send(&eth);
                }
            } else if mlme::ends(&radio, l, f) {
                link = None;
            }
        }
        // What netd sends on the link.
        if let (Some(t), Some(l)) = (&tap, &link) {
            while let Some(n) = t.recv(&mut frame) {
                if let Some(f) = frame::data_to_ds(radio.mac(), l.bss.bssid, &frame[..n]) {
                    radio.send(&f);
                }
            }
        }
        while let Some(conn) = listener.accept() {
            let mut buf = [0u8; 128];
            let n = conn.read(&mut buf, 2000);
            let line = buf[..n].trim_ascii();
            let (cmd, arg) = match line.iter().position(|&c| c == b' ') {
                Some(i) => (&line[..i], line[i + 1..].trim_ascii()),
                None => (line, &[][..]),
            };
            // Another network, or none: netd lets go of the old link's
            // NIC (and its lease) first.
            if matches!(cmd, b"connect" | b"disconnect") {
                if let Some(t) = tap.take() {
                    t.close();
                }
            }
            let reply = match cmd {
                b"scan" => {
                    seen = scan(&mut radio);
                    listing(&seen)
                }
                b"list" => listing(&seen),
                b"connect" => connect(&mut radio, &mut seen, &mut link, arg),
                b"status" => status(link.as_ref()),
                b"disconnect" => {
                    if let Some(l) = link.take() {
                        mlme::leave(&mut radio, &l);
                    }
                    status(None)
                }
                _ => Vec::from(b"error: unknown command\n" as &[u8]),
            };
            conn.write(&reply);
            conn.close();
            // A scan leaves the radio on the last channel.
            if let Some(l) = &link {
                radio.set_channel(l.bss.channel);
            }
        }
        match (&link, tap.is_some()) {
            (Some(_), false) => {
                tap = Tap::open(radio.mac());
                if tap.is_none() {
                    write(b"wifi: no virtual NIC for the link (/dev/tap)\n");
                }
            }
            (None, true) => tap.take().unwrap().close(),
            _ => {}
        }
    }
}

/// `connect SSID`: leave the current network, then join the strongest
/// SSID heard (scanning first when the last scan did not hear it).
fn connect(radio: &mut Sim, seen: &mut Vec<Seen>, link: &mut Option<Link>, ssid: &[u8]) -> Vec<u8> {
    if ssid.is_empty() {
        return Vec::from(b"error: connect SSID\n" as &[u8]);
    }
    if let Some(l) = link.take() {
        mlme::leave(radio, &l);
    }
    if !seen.iter().any(|s| s.bss.ssid == ssid) {
        *seen = scan(radio);
    }
    // `seen` is the strongest first.
    let mut e = Vec::from(b"error: " as &[u8]);
    let Some(bss) = seen.iter().find(|s| s.bss.ssid == ssid).map(|s| s.bss.clone()) else {
        e.extend_from_slice(b"no network ");
        ssid_text(&mut e, ssid);
        e.push(b'\n');
        return e;
    };
    if bss.security != Security::Open {
        ssid_text(&mut e, ssid);
        e.extend_from_slice(b" is ");
        e.extend_from_slice(bss.security.name());
        e.extend_from_slice(b": only open networks so far\n");
        return e;
    }
    match mlme::join(radio, bss) {
        Ok(l) => {
            *link = Some(l);
            return status(link.as_ref());
        }
        Err(Refused::Silent(step)) => {
            e.extend_from_slice(b"no answer to the ");
            e.extend_from_slice(step);
            e.extend_from_slice(b" request\n");
        }
        Err(Refused::Status(step, code)) => {
            e.extend_from_slice(step);
            e.extend_from_slice(b" refused, status ");
            dec(&mut e, i64::from(code));
            e.push(b'\n');
        }
    }
    e
}

/// `associated BSSID CHANNEL SSID aid N`, or `disconnected`.
fn status(link: Option<&Link>) -> Vec<u8> {
    let Some(l) = link else {
        return Vec::from(b"disconnected\n" as &[u8]);
    };
    let mut out = Vec::from(b"associated " as &[u8]);
    mac_text(&mut out, l.bss.bssid);
    out.push(b' ');
    dec(&mut out, i64::from(l.bss.channel));
    out.push(b' ');
    ssid_text(&mut out, &l.bss.ssid);
    out.extend_from_slice(b" aid ");
    dec(&mut out, i64::from(l.aid));
    out.push(b'\n');
    out
}

/// Visit every channel: a wildcard probe request, then whatever beacons and
/// probe responses are heard for [`DWELL_MS`].
fn scan(radio: &mut dyn Radio) -> Vec<Seen> {
    let mut seen: Vec<Seen> = Vec::new();
    let mut buf = [0u8; 2048];
    for ch in CHANNELS {
        radio.set_channel(ch);
        radio.send(&frame::probe_request(radio.mac()));
        let end = monotonic_ns() + DWELL_MS * 1_000_000;
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
            let left = end.saturating_sub(monotonic_ns());
            if left == 0 {
                break;
            }
            let mut fds = [PollFd { fd: radio.fd() as i32, events: POLLIN, revents: 0 }];
            poll(&mut fds, (left / 1_000_000).max(1) as i32);
        }
    }
    seen.sort_by(|a, b| b.rssi.cmp(&a.rssi));
    seen
}

fn listing(seen: &[Seen]) -> Vec<u8> {
    let mut out = Vec::new();
    for s in seen {
        mac_text(&mut out, s.bss.bssid);
        out.push(b' ');
        dec(&mut out, i64::from(s.bss.channel));
        out.push(b' ');
        dec(&mut out, i64::from(s.rssi));
        out.push(b' ');
        out.extend_from_slice(s.bss.security.name());
        out.push(b' ');
        ssid_text(&mut out, &s.bss.ssid);
        out.push(b'\n');
    }
    out
}

fn mac_text(out: &mut Vec<u8>, mac: [u8; 6]) {
    for (i, b) in mac.iter().enumerate() {
        if i != 0 {
            out.push(b':');
        }
        hex(out, *b);
    }
}

/// An SSID's bytes, those outside printable ASCII as `\\xHH`.
fn ssid_text(out: &mut Vec<u8>, ssid: &[u8]) {
    for &c in ssid {
        if (0x21..0x7f).contains(&c) && c != b'\\' {
            out.push(c);
        } else {
            out.extend_from_slice(b"\\x");
            hex(out, c);
        }
    }
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

/// Ask the daemon: the command and its arguments out as a line, its answer
/// to stdout until it ends the connection.
fn client() -> ! {
    let mut connect = Vec::from(b"connect " as &[u8]);
    connect.extend_from_slice(unix::NAME);
    let Some(conn) = Conv::new(&connect) else {
        write(b"wifi: no daemon (wifi daemon)\n");
        exit_code(1);
    };
    let mut line = Vec::new();
    for i in 1..myos_user::args::argc() {
        if i > 1 {
            line.push(b' ');
        }
        line.extend_from_slice(myos_user::args::arg(i).unwrap_or(b""));
    }
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
