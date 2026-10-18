//! `wifi`: the Wi-Fi daemon and its command (issue #495, `docs/wifi.md`).
//!
//! - `wifi daemon [NIC]` drives the radio: the AX201 when `/dev/pci` has
//!   one and no NIC is named (its firmware from `wifi-firmware fetch`),
//!   else the simulated one on NIC (default `/dev/net1`, the test boots'
//!   second NIC, `src/wifi_sim.rs`).
//!   It listens on the unix socket `wifid` for the commands below, one per
//!   connection: a line in, the answer out, then the connection ends.
//! - `wifi scan`: a scan of the 2.4 GHz channels; prints what answered.
//! - `wifi list`: what the last scan found.
//! - `wifi connect SSID [PASSPHRASE]`: join the network SSID (the
//!   strongest the last scan heard, after a scan when it heard none): an
//!   open one, or a WPA2-PSK one with its passphrase (or the PSK as 64 hex
//!   digits).
//! - `wifi status`: `associated BSSID CHANNEL SSID aid N`, or
//!   `disconnected`.
//! - `wifi disconnect`: leave the network.
//!
//! While associated, the link is a virtual NIC of `/dev/tap` for netd
//! (`tapN` in `/net/ifaddrs`, its address from the network's DHCP): the
//! daemon carries its Ethernet frames as 802.11 data frames, encrypted
//! with CCMP on a WPA2 network.
//!
//! A network is one line: `BSSID CHANNEL RSSI SECURITY SSID`, the strongest
//! first (`02:00:5e:00:01:01 1 -40 open myos-open`); bytes of the SSID
//! outside printable ASCII are written `\xHH`.

#![no_std]
#![deny(unsafe_op_in_unsafe_fn)]
#![no_main]

extern crate alloc;

mod ax201;
mod frame;
mod mlme;
mod pci;
mod radio;
mod rsn;
mod tap;
mod unix;

use alloc::boxed::Box;
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
    write(b"usage: wifi daemon [NIC] | wifi scan | list | connect SSID [PASSPHRASE] | status | disconnect\n");
    exit_code(2);
}

fn main() -> ! {
    heap_init();
    match myos_user::args::arg(1) {
        Some(b"daemon") => daemon(myos_user::args::arg(2)),
        Some(b"scan" | b"list" | b"connect" | b"status" | b"disconnect") => client(),
        _ => usage(),
    }
}

/// A network a scan heard.
struct Seen {
    bss: Bss,
    rssi: i8,
}

/// The radio: NIC's simulated one when named, else the AX201 when there
/// is one, else the simulated one on `/dev/net1`.
fn open_radio(nic: Option<&[u8]>) -> Option<Box<dyn Radio>> {
    if nic.is_none()
        && let Some(dir) = ax201::find()
    {
        return match ax201::Ax201::open(&dir) {
            Ok(r) => Some(Box::new(r)),
            Err(f) => {
                write(&ax201::why(&f));
                None
            }
        };
    }
    Some(Box::new(Sim::open(nic.unwrap_or(b"/dev/net1"))?))
}

fn daemon(nic: Option<&[u8]>) -> ! {
    let Some(mut radio) = open_radio(nic) else {
        write(b"wifi: no radio\n");
        exit_code(1);
    };
    let radio = &mut *radio;
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
        // What the radio hears between commands: data for the link's NIC
        // (decrypted first on a WPA2 network, where the access point may
        // also start a group key handshake); the access point letting the
        // station go ends the link.
        while let Some(rx) = radio.recv(&mut frame) {
            let Some(l) = link.as_mut() else {
                continue;
            };
            let f = &frame[..rx.len];
            if mlme::ends(radio, l, f) {
                link = None;
                continue;
            }
            let (me, bssid) = (radio.mac(), l.bss.bssid);
            let clear;
            let f = match l.rsn.as_mut() {
                Some(r) => match r.open(f) {
                    Some(c) => {
                        clear = c;
                        &clear[..]
                    }
                    None => continue,
                },
                None => f,
            };
            if let (Some(r), Some(pdu)) = (l.rsn.as_mut(), rsn::eapol_of(f, me, bssid)) {
                r.eapol(radio, bssid, &pdu);
            } else if let (Some(eth), Some(t)) = (frame::data_from_ds(f, me, bssid), &tap) {
                t.send(&eth);
            }
        }
        // What netd sends on the link.
        if let (Some(t), Some(l)) = (&tap, &mut link) {
            while let Some(n) = t.recv(&mut frame) {
                let Some(f) = frame::data_to_ds(radio.mac(), l.bss.bssid, &frame[..n]) else {
                    continue;
                };
                match l.rsn.as_mut() {
                    Some(r) => {
                        if let Some(f) = r.seal(&f) {
                            radio.send(&f);
                        }
                    }
                    None => {
                        radio.send(&f);
                    }
                }
            }
        }
        while let Some(conn) = listener.accept() {
            let mut buf = [0u8; 192];
            let n = conn.read(&mut buf, 2000);
            // The command line, then for `connect` the passphrase's line.
            let (line, passphrase) = match buf[..n].iter().position(|&c| c == b'\n') {
                Some(i) => (buf[..i].trim_ascii(), buf[i + 1..n].strip_suffix(b"\n").unwrap_or(&buf[i + 1..n])),
                None => (buf[..n].trim_ascii(), &[][..]),
            };
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
                    seen = scan(radio);
                    listing(&seen)
                }
                b"list" => listing(&seen),
                b"connect" => connect(radio, &mut seen, &mut link, arg, passphrase),
                b"status" => status(link.as_ref()),
                b"disconnect" => {
                    if let Some(l) = link.take() {
                        mlme::leave(radio, &l);
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

/// `connect SSID [PASSPHRASE]`: leave the current network, then join the
/// strongest SSID heard (scanning first when the last scan did not hear
/// it); a WPA2-PSK one with its passphrase.
fn connect(
    radio: &mut dyn Radio,
    seen: &mut Vec<Seen>,
    link: &mut Option<Link>,
    ssid: &[u8],
    passphrase: &[u8],
) -> Vec<u8> {
    if ssid.is_empty() {
        return Vec::from(b"error: connect SSID\n" as &[u8]);
    }
    if !radio.joins() {
        return Vec::from(b"error: this radio scans only so far (issue #495)\n" as &[u8]);
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
    let pmk = match bss.security {
        Security::Open => None,
        Security::Wpa2Psk if passphrase.is_empty() => {
            ssid_text(&mut e, ssid);
            e.extend_from_slice(b" is wpa2-psk: connect SSID PASSPHRASE\n");
            return e;
        }
        Security::Wpa2Psk => match wpa::psk(passphrase, &bss.ssid) {
            Some(pmk) => Some(pmk),
            None => {
                e.extend_from_slice(b"a passphrase is 8 to 63 printable ASCII characters, or 64 hex digits\n");
                return e;
            }
        },
        _ => {
            ssid_text(&mut e, ssid);
            e.extend_from_slice(b" is ");
            e.extend_from_slice(bss.security.name());
            e.extend_from_slice(b": only open and wpa2-psk networks so far\n");
            return e;
        }
    };
    let rsn = if pmk.is_some() { &wpa::RSN_PSK_CCMP[..] } else { &[][..] };
    let joined = mlme::join(radio, bss, rsn).and_then(|mut l| {
        if let Some(pmk) = pmk {
            match rsn::handshake(radio, &l.bss, pmk) {
                Ok(r) => l.rsn = Some(r),
                Err(why) => {
                    mlme::leave(radio, &l);
                    return Err(why);
                }
            }
        }
        Ok(l)
    });
    match joined {
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
        Err(Refused::Handshake(why)) => {
            e.extend_from_slice(b"4-way handshake: ");
            e.extend_from_slice(why);
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
/// probe responses are heard for [`DWELL_MS`]; or the radio's own scan.
fn scan(radio: &mut dyn Radio) -> Vec<Seen> {
    let mut seen: Vec<Seen> = Vec::new();
    if let Some(heard) = radio.scan() {
        for h in heard {
            if let Some(bss) = frame::parse_bss(&h.frame, h.channel) {
                hear(&mut seen, bss, h.rssi);
            }
        }
        seen.sort_by(|a, b| b.rssi.cmp(&a.rssi));
        return seen;
    }
    let mut buf = [0u8; 2048];
    for ch in CHANNELS {
        radio.set_channel(ch);
        radio.send(&frame::probe_request(radio.mac()));
        let end = monotonic_ns() + DWELL_MS * 1_000_000;
        loop {
            while let Some(rx) = radio.recv(&mut buf) {
                if let Some(bss) = frame::parse_bss(&buf[..rx.len], rx.channel) {
                    hear(&mut seen, bss, rx.rssi);
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

/// A network heard, at its strongest.
fn hear(seen: &mut Vec<Seen>, bss: Bss, rssi: i8) {
    match seen.iter_mut().find(|s| s.bss.bssid == bss.bssid) {
        Some(s) => s.rssi = s.rssi.max(rssi),
        None => seen.push(Seen { bss, rssi }),
    }
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
    // The command and its argument; `connect`'s passphrase on a line of
    // its own (an SSID may have spaces in it, and so may a passphrase).
    let arg = |i| myos_user::args::arg(i).unwrap_or(b"");
    let argc = myos_user::args::argc();
    let mut line = Vec::from(arg(1));
    if argc > 2 {
        line.push(b' ');
        line.extend_from_slice(arg(2));
    }
    line.push(b'\n');
    if arg(1) == b"connect" && argc > 3 {
        line.extend_from_slice(arg(3));
        line.push(b'\n');
    }
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
