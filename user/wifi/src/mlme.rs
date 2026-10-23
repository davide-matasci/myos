//! Joining a network: open-system authentication, then association, each
//! a request and its answer, sent again when no answer comes (then, on a
//! WPA2 network, the 4-way handshake: `rsn.rs`); leaving it with a
//! deauthentication. The radio is told each step (its firmware, the
//! Intel radio's, keeps the network's state).

use crate::frame::{self, Bss};
use crate::radio::Radio;
use crate::rsn::Rsn;
use myos_user::{monotonic_ns, poll, PollFd, POLLIN};

/// How long a request waits for its answer, and how often it is sent.
const ANSWER_MS: u64 = 300;
const TRIES: usize = 3;

/// The network the station is associated with.
pub struct Link {
    pub bss: Bss,
    pub aid: u16,
    /// A WPA2 network's keys.
    pub rsn: Option<Rsn>,
}

/// Why joining failed.
pub enum Refused {
    /// No answer to a request.
    Silent(&'static [u8]),
    /// The access point refused a request with this status code.
    Status(&'static [u8], u16),
    /// The 4-way handshake failed: why.
    Handshake(&'static [u8]),
    /// The radio could not take the step.
    Radio(&'static [u8]),
}

/// Authenticate with and associate to `bss`, offering RSN element `rsn`
/// (whole; empty for an open network).
pub fn join(radio: &mut dyn Radio, bss: Bss, rsn: &[u8]) -> Result<Link, Refused> {
    let r = join_steps(radio, bss, rsn);
    if r.is_err() {
        radio.left();
    }
    r
}

fn join_steps(radio: &mut dyn Radio, bss: Bss, rsn: &[u8]) -> Result<Link, Refused> {
    let me = radio.mac();
    radio.set_channel(bss.channel);
    if !radio.joining(&bss) {
        return Err(Refused::Radio(b"authentication"));
    }
    let auth = frame::auth_request(me, bss.bssid);
    let status = exchange(radio, &auth, bss.bssid, frame::AUTH, frame::auth_status)
        .ok_or(Refused::Silent(b"authentication"))?;
    if status != frame::STATUS_OK {
        return Err(Refused::Status(b"authentication", status));
    }
    let assoc = frame::assoc_request(me, bss.bssid, &bss.ssid, rsn);
    let (status, aid) = exchange(radio, &assoc, bss.bssid, frame::ASSOC_RESP, frame::assoc_status)
        .ok_or(Refused::Silent(b"association"))?;
    if status != frame::STATUS_OK {
        return Err(Refused::Status(b"association", status));
    }
    if !radio.associated(aid) {
        return Err(Refused::Radio(b"association"));
    }
    Ok(Link { bss, aid, rsn: None })
}

/// Leave the network.
pub fn leave(radio: &mut dyn Radio, link: &Link) {
    radio.send(&frame::deauth(radio.mac(), link.bss.bssid, frame::REASON_LEAVING));
    radio.left();
}

/// Whether `f` ends the link: the access point's deauthentication or
/// disassociation of this station.
pub fn ends(radio: &dyn Radio, link: &Link, f: &[u8]) -> bool {
    matches!(frame::from_ap(f, radio.mac(), link.bss.bssid), Some((frame::DEAUTH | frame::DISASSOC, _)))
}

/// Send `request` until the access point answers with a `subtype` frame
/// that `parse` reads, up to [`TRIES`] times.
fn exchange<T>(
    radio: &mut dyn Radio,
    request: &[u8],
    bssid: [u8; 6],
    subtype: u8,
    parse: fn(&[u8]) -> Option<T>,
) -> Option<T> {
    let me = radio.mac();
    let mut buf = [0u8; 2048];
    for _ in 0..TRIES {
        radio.send(request);
        let end = monotonic_ns() + ANSWER_MS * 1_000_000;
        loop {
            while let Some(rx) = radio.recv(&mut buf) {
                if let Some((st, body)) = frame::from_ap(&buf[..rx.len], me, bssid) {
                    if let Some(v) = (st == subtype).then(|| parse(body)).flatten() {
                        return Some(v);
                    }
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
    None
}
