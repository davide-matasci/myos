//! `wifi`: the Wi-Fi daemon and its command (issue #495, `docs/wifi.md`).
//!
//! - `wifi daemon [NIC]` drives the radio: an Intel AX201 or AX200 when
//!   `/dev/pci` has one and no NIC is named, else a MediaTek MT7601U or
//!   MT7612U when `/dev/usb` has one, else a Broadcom CYW43455 or CYW43438
//!   (the Raspberry Pi's) when `/dev/sdio` has one (their firmware from
//!   `wifi-firmware fetch`), else the simulated one on NIC (default
//!   `/dev/net1`, the test boots' second NIC, `src/wifi_sim.rs`).
//!   It listens on the unix socket `wifid` for the commands below, one per
//!   connection: a line in, the answer out, then the connection ends.
//! - `wifi scan`: a scan of the radio's channels (2.4 GHz, and 5 GHz on a
//!   dual-band one); prints what answered.
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
//! with CCMP on a WPA2 network; a FullMAC radio (Broadcom's) does that
//! itself and takes the Ethernet frames.
//!
//! A network is one line: `BSSID CHANNEL RSSI SECURITY SSID`, the strongest
//! first (`02:00:5e:00:01:01 1 -40 open myos-open`); bytes of the SSID
//! outside printable ASCII are written `\xHH`.

#![no_std]
#![deny(unsafe_op_in_unsafe_fn)]
#![no_main]

extern crate alloc;

mod broadcom;
mod devs;
mod frame;
mod intel;
mod mediatek;
mod mlme;
mod pci;
mod radio;
mod rsn;
mod sdio;
mod tap;
mod unix;
mod usb;

use alloc::boxed::Box;
use alloc::vec::Vec;

use frame::{Bss, Security};
use mlme::{Link, Refused};
use myos_user::{exit_code, heap_init, monotonic_ns, poll, write, Heap, PollFd, POLLIN};
use radio::{FullMac, Radio, Seen, Sim};
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

/// How long a scan listens on each channel after its probe request, in
/// ms: beacons come every 100 TU (102.4 ms), so one is heard too.
const DWELL_MS: u64 = 120;
/// How often a FullMAC radio without an interrupt line is looked at, ms.
const LOOK_MS: i32 = 20;

/// Whether 5 GHz channel `ch` is one where radar has precedence (DFS,
/// UNII-2A and -2C): a station only listens there until it hears an
/// access point.
fn radar(ch: u8) -> bool {
    (52..=144).contains(&ch)
}

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

/// The radio found: one the daemon scans and joins with, or one whose
/// firmware does.
enum Found {
    Soft(Box<dyn Radio>),
    Full(Box<dyn FullMac>),
}

/// The radio: NIC's simulated one when named, else an Intel one when
/// there is one, else a MediaTek one, else a Broadcom one, else the
/// simulated one on `/dev/net1`.
fn open_radio(nic: Option<&[u8]>) -> Option<Found> {
    if nic.is_none()
        && let Some((dir, chip)) = intel::find()
    {
        return match intel::Intel::open(&dir, chip) {
            Ok(r) => Some(Found::Soft(Box::new(r))),
            Err(f) => {
                write(&intel::why(chip, &f));
                None
            }
        };
    }
    if nic.is_none()
        && let Some((dir, chip)) = mediatek::find()
    {
        return match mediatek::Mediatek::open(&dir, chip) {
            Ok(r) => Some(Found::Soft(Box::new(r))),
            Err(f) => {
                write(&mediatek::why(chip, &f));
                None
            }
        };
    }
    if nic.is_none()
        && let Some((dir, chip)) = broadcom::find_radio()
    {
        return match broadcom::Broadcom::open(&dir, chip) {
            Ok(r) => Some(Found::Full(Box::new(r))),
            Err(f) => {
                write(&broadcom::why(chip, &f));
                None
            }
        };
    }
    Some(Found::Soft(Box::new(Sim::open(nic.unwrap_or(b"/dev/net1"))?)))
}

fn daemon(nic: Option<&[u8]>) -> ! {
    let Some(radio) = open_radio(nic) else {
        write(b"wifi: no radio\n");
        exit_code(1);
    };
    let mut cmd = Vec::from(b"announce " as &[u8]);
    cmd.extend_from_slice(unix::NAME);
    let Some(listener) = Conv::new(&cmd) else {
        write(b"wifi: the socket wifid is taken\n");
        exit_code(1);
    };
    match radio {
        Found::Soft(mut r) => daemon_softmac(&mut *r, listener),
        Found::Full(mut r) => daemon_fullmac(&mut *r, listener),
    }
}

/// A command's line and `connect`'s passphrase line, read from `conn`.
fn command(conn: &Conv) -> (Vec<u8>, Vec<u8>, Vec<u8>) {
    let mut buf = [0u8; 192];
    let n = conn.read(&mut buf, 2000);
    let (line, passphrase) = match buf[..n].iter().position(|&c| c == b'\n') {
        Some(i) => (buf[..i].trim_ascii(), buf[i + 1..n].strip_suffix(b"\n").unwrap_or(&buf[i + 1..n])),
        None => (buf[..n].trim_ascii(), &[][..]),
    };
    let (cmd, arg) = match line.iter().position(|&c| c == b' ') {
        Some(i) => (&line[..i], line[i + 1..].trim_ascii()),
        None => (line, &[][..]),
    };
    (cmd.to_vec(), arg.to_vec(), passphrase.to_vec())
}

/// The link's NIC follows the link: opened when a link comes, closed
/// when it goes.
fn keep_tap(tap: &mut Option<Tap>, linked: bool, mac: [u8; 6]) {
    match (linked, tap.is_some()) {
        (true, false) => {
            *tap = Tap::open(mac);
            if tap.is_none() {
                write(b"wifi: no virtual NIC for the link (/dev/tap)\n");
            }
        }
        (false, true) => tap.take().unwrap().close(),
        _ => {}
    }
}

/// The daemon on a radio it scans and joins with.
fn daemon_softmac(radio: &mut dyn Radio, listener: Conv) -> ! {
    let mut seen: Vec<Seen> = Vec::new();
    let mut link: Option<Link> = None;
    // The link's NIC, there while `link` is.
    let mut tap: Option<Tap> = None;
    let mut frame = [0u8; 2048];
    let mut tick = monotonic_ns();
    loop {
        let mut fds = Vec::from([
            PollFd { fd: listener.data as i32, events: POLLIN, revents: 0 },
            PollFd { fd: radio.fd() as i32, events: POLLIN, revents: 0 },
        ]);
        if let Some(t) = &tap {
            fds.push(PollFd { fd: t.fd() as i32, events: POLLIN, revents: 0 });
        }
        // A link wakes the loop every second for the radio's tick.
        let wait = if radio.pending() {
            0
        } else if link.is_some() {
            1000
        } else {
            -1
        };
        poll(&mut fds, wait);
        if link.is_some() && monotonic_ns() >= tick + 1_000_000_000 {
            tick = monotonic_ns();
            radio.tick();
        }
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
                radio.left();
                continue;
            }
            let (me, bssid) = (radio.mac(), l.bss.bssid);
            // A block ack session the access point offers: declined.
            if let Some((frame::ACTION, body)) = frame::from_ap(f, me, bssid) {
                if let Some(answer) = frame::addba_declined(me, bssid, body) {
                    radio.send(&answer);
                }
                continue;
            }
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
                let Some(f) = frame::data_to_ds(radio.mac(), l.bss.bssid, &frame[..n], l.qos) else {
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
            let (cmd, arg, passphrase) = command(&conn);
            // Another network, or none: netd lets go of the old link's
            // NIC (and its lease) first.
            if matches!(&cmd[..], b"connect" | b"disconnect") {
                if let Some(t) = tap.take() {
                    t.close();
                }
            }
            let reply = match &cmd[..] {
                b"scan" => {
                    seen = scan(radio);
                    listing(&seen)
                }
                b"list" => listing(&seen),
                b"connect" => connect(radio, &mut seen, &mut link, &arg, &passphrase),
                b"status" => status(link.as_ref().map(|l| (&l.bss, l.aid))),
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
        keep_tap(&mut tap, link.is_some(), radio.mac());
    }
}

/// The daemon on a radio whose firmware scans, joins and encrypts: the
/// link's Ethernet frames go between it and the NIC as they are.
fn daemon_fullmac(radio: &mut dyn FullMac, listener: Conv) -> ! {
    let mut seen: Vec<Seen> = Vec::new();
    let mut link: Option<Bss> = None;
    let mut tap: Option<Tap> = None;
    let mut frame = [0u8; 2048];
    loop {
        let mut fds = Vec::from([PollFd { fd: listener.data as i32, events: POLLIN, revents: 0 }]);
        if let Some(fd) = radio.fd() {
            fds.push(PollFd { fd: fd as i32, events: POLLIN, revents: 0 });
        }
        if let Some(t) = &tap {
            fds.push(PollFd { fd: t.fd() as i32, events: POLLIN, revents: 0 });
        }
        poll(&mut fds, if radio.pending() { 0 } else if radio.fd().is_some() { -1 } else { LOOK_MS });
        radio.service();
        // The access point letting the station go ends the link.
        if link.is_some() && !radio.up() {
            link = None;
        }
        while let Some(eth) = radio.recv() {
            if let (Some(_), Some(t)) = (&link, &tap) {
                t.send(&eth);
            }
        }
        if let (Some(t), Some(_)) = (&tap, &link) {
            while let Some(n) = t.recv(&mut frame) {
                radio.send(&frame[..n]);
            }
        }
        while let Some(conn) = listener.accept() {
            let (cmd, arg, passphrase) = command(&conn);
            if matches!(&cmd[..], b"connect" | b"disconnect") {
                if let Some(t) = tap.take() {
                    t.close();
                }
            }
            let reply = match &cmd[..] {
                b"scan" => match radio.scan() {
                    Ok(s) => {
                        seen = s;
                        listing(&seen)
                    }
                    Err(why) => error(why.as_bytes()),
                },
                b"list" => listing(&seen),
                b"connect" => connect_fullmac(radio, &mut seen, &mut link, &arg, &passphrase),
                // The firmware keeps the association: no aid to show.
                b"status" => status(link.as_ref().map(|b| (b, 0))),
                b"disconnect" => {
                    if link.take().is_some() {
                        radio.leave();
                    }
                    status(None)
                }
                _ => Vec::from(b"error: unknown command\n" as &[u8]),
            };
            conn.write(&reply);
            conn.close();
        }
        keep_tap(&mut tap, link.is_some(), radio.mac());
    }
}

/// An `error: WHY` line.
fn error(why: &[u8]) -> Vec<u8> {
    let mut e = Vec::from(b"error: " as &[u8]);
    e.extend_from_slice(why);
    e.push(b'\n');
    e
}

/// The strongest network `ssid` in `seen` (the strongest first), or the
/// error that there is none.
fn pick(seen: &[Seen], ssid: &[u8]) -> Result<Bss, Vec<u8>> {
    seen.iter().find(|s| s.bss.ssid == ssid).map(|s| s.bss.clone()).ok_or_else(|| {
        let mut e = Vec::from(b"no network " as &[u8]);
        ssid_text(&mut e, ssid);
        error(&e)
    })
}

/// The pairwise master key for `bss` from `passphrase`: none on an open
/// network; the error when the network needs one the user did not give,
/// gave wrongly formed, or is secured another way.
fn pmk_for(bss: &Bss, passphrase: &[u8]) -> Result<Option<[u8; 32]>, Vec<u8>> {
    let mut e = Vec::new();
    match bss.security {
        Security::Open => Ok(None),
        Security::Wpa2Psk if passphrase.is_empty() => {
            ssid_text(&mut e, &bss.ssid);
            e.extend_from_slice(b" is wpa2-psk: connect SSID PASSPHRASE");
            Err(error(&e))
        }
        Security::Wpa2Psk => match wpa::psk(passphrase, &bss.ssid) {
            Some(pmk) => Ok(Some(pmk)),
            None => Err(error(b"a passphrase is 8 to 63 printable ASCII characters, or 64 hex digits")),
        },
        _ => {
            ssid_text(&mut e, &bss.ssid);
            e.extend_from_slice(b" is ");
            e.extend_from_slice(bss.security.name());
            e.extend_from_slice(b": only open and wpa2-psk networks so far");
            Err(error(&e))
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
        return error(b"connect SSID");
    }
    if let Some(l) = link.take() {
        mlme::leave(radio, &l);
    }
    if !seen.iter().any(|s| s.bss.ssid == ssid) {
        *seen = scan(radio);
    }
    let bss = match pick(seen, ssid) {
        Ok(bss) => bss,
        Err(e) => return e,
    };
    let pmk = match pmk_for(&bss, passphrase) {
        Ok(pmk) => pmk,
        Err(e) => return e,
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
    let mut e = Vec::from(b"error: " as &[u8]);
    match joined {
        Ok(l) => {
            *link = Some(l);
            return status(link.as_ref().map(|l| (&l.bss, l.aid)));
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
        Err(Refused::Radio(step)) => {
            e.extend_from_slice(b"the radio failed to set up the ");
            e.extend_from_slice(step);
            e.push(b'\n');
        }
    }
    e
}

/// `connect` on a FullMAC radio: the firmware joins (and, given the
/// pairwise master key, runs the 4-way handshake).
fn connect_fullmac(
    radio: &mut dyn FullMac,
    seen: &mut Vec<Seen>,
    link: &mut Option<Bss>,
    ssid: &[u8],
    passphrase: &[u8],
) -> Vec<u8> {
    if ssid.is_empty() {
        return error(b"connect SSID");
    }
    if link.take().is_some() {
        radio.leave();
    }
    if !seen.iter().any(|s| s.bss.ssid == ssid) {
        match radio.scan() {
            Ok(s) => *seen = s,
            Err(why) => return error(why.as_bytes()),
        }
    }
    let bss = match pick(seen, ssid) {
        Ok(bss) => bss,
        Err(e) => return e,
    };
    let pmk = match pmk_for(&bss, passphrase) {
        Ok(pmk) => pmk,
        Err(e) => return e,
    };
    match radio.join(&bss, pmk.as_ref()) {
        Ok(()) => {
            *link = Some(bss);
            status(link.as_ref().map(|b| (b, 0)))
        }
        Err(why) => error(why.as_bytes()),
    }
}

/// `associated BSSID CHANNEL SSID aid N`, or `disconnected`.
fn status(link: Option<(&Bss, u16)>) -> Vec<u8> {
    let Some((bss, aid)) = link else {
        return Vec::from(b"disconnected\n" as &[u8]);
    };
    let mut out = Vec::from(b"associated " as &[u8]);
    mac_text(&mut out, bss.bssid);
    out.push(b' ');
    dec(&mut out, i64::from(bss.channel));
    out.push(b' ');
    ssid_text(&mut out, &bss.ssid);
    out.extend_from_slice(b" aid ");
    dec(&mut out, i64::from(aid));
    out.push(b'\n');
    out
}

/// Visit every channel of the radio: a wildcard probe request (none on a
/// radar channel), then whatever beacons and probe responses are heard
/// for [`DWELL_MS`]; or the radio's own scan.
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
    for ch in radio.channels() {
        radio.set_channel(ch);
        if !radar(ch) {
            radio.send(&frame::probe_request(radio.mac(), ch));
        }
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
        // A scan takes the radio's channels × DWELL_MS (38 channels on a
        // dual-band radio: under 5 s), well within the wait.
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
