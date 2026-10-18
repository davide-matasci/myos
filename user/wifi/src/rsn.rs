//! A WPA2-PSK link's keys (the `wpa` crate): the 4-way handshake right
//! after association, then CCMP on every data frame, and the group key
//! handshakes the access point starts while the link is up.

use alloc::vec::Vec;

use myos_user::{close, monotonic_ns, open_flags, poll, read, PollFd, O_RDONLY, POLLIN};
use wpa::ccmp;
use wpa::eapol::{self, Step, Supplicant};

use crate::frame::{self, Bss};
use crate::mlme::Refused;
use crate::radio::Radio;

/// How long the handshake may take, from association to message 4.
const HANDSHAKE_MS: u64 = 3000;

/// The keys of a link: the pairwise one and the group ones by index, each
/// with the last packet number heard (a frame not past it is a replay).
pub struct Rsn {
    supplicant: Supplicant,
    tk: [u8; 16],
    tx_pn: u64,
    rx_pn: u64,
    groups: [Option<([u8; 16], u64)>; 4],
}

/// The 4-way handshake with `bss`, which the station just associated to
/// with [`wpa::RSN_PSK_CCMP`]: the access point's EAPOL-Key messages
/// answered until the keys are in.
pub fn handshake(radio: &mut dyn Radio, bss: &Bss, pmk: [u8; 32]) -> Result<Rsn, Refused> {
    let me = radio.mac();
    let snonce = random().ok_or(Refused::Handshake(b"no random numbers (/dev/urandom)"))?;
    let mut s = Supplicant::new(pmk, bss.bssid, me, &wpa::RSN_PSK_CCMP, &bss.rsn, snonce);
    let mut buf = [0u8; 2048];
    let mut answered = false;
    let end = monotonic_ns() + HANDSHAKE_MS * 1_000_000;
    loop {
        while let Some(rx) = radio.recv(&mut buf) {
            let f = &buf[..rx.len];
            if matches!(frame::from_ap(f, me, bss.bssid), Some((frame::DEAUTH | frame::DISASSOC, _))) {
                // After message 2, the access point's answer to a MIC
                // made with the wrong key.
                return Err(Refused::Handshake(if answered {
                    b"the access point refused the key: wrong passphrase?"
                } else {
                    b"the access point let the station go"
                }));
            }
            let Some(pdu) = eapol_of(f, me, bss.bssid) else {
                continue;
            };
            match s.input(&pdu) {
                Ok(Step::Ignore) => {}
                Ok(Step::Reply(m)) => {
                    send_eapol(radio, bss.bssid, &m, None);
                    answered = true;
                }
                Ok(Step::Installed(m)) => {
                    // Message 4 goes out in the clear: the access point
                    // installs its keys only once it has it.
                    send_eapol(radio, bss.bssid, &m, None);
                    let k = s.keys().unwrap().clone();
                    let mut groups = [None; 4];
                    groups[usize::from(k.gtk.0)] = Some((k.gtk.1, k.gtk.2));
                    return Ok(Rsn { supplicant: s, tk: k.tk, tx_pn: 0, rx_pn: 0, groups });
                }
                Err(eapol::Error::Mic) => return Err(Refused::Handshake(b"wrong passphrase (message 3's MIC)")),
                Err(eapol::Error::Rsn) => {
                    return Err(Refused::Handshake(b"the access point's RSN element changed (a downgrade?)"));
                }
                Err(eapol::Error::KeyData) => return Err(Refused::Handshake(b"message 3 had no group key")),
            }
        }
        let left = end.saturating_sub(monotonic_ns());
        if left == 0 {
            return Err(Refused::Handshake(if answered {
                b"no message 3 from the access point: wrong passphrase?"
            } else {
                b"no message 1 from the access point"
            }));
        }
        let mut fds = [PollFd { fd: radio.fd() as i32, events: POLLIN, revents: 0 }];
        poll(&mut fds, (left / 1_000_000).max(1) as i32);
    }
}

impl Rsn {
    /// Data frame `f`, encrypted with the pairwise key.
    pub fn seal(&mut self, f: &[u8]) -> Option<Vec<u8>> {
        self.tx_pn += 1;
        ccmp::encrypt(&self.tk, self.tx_pn, 0, f)
    }

    /// Protected data frame `f` in the clear: decrypted with the pairwise
    /// key (index 0) or a group key; `None` when it is not protected, its
    /// key is not known, its MIC is wrong, or it is a replay.
    pub fn open(&mut self, f: &[u8]) -> Option<Vec<u8>> {
        let (idx, _) = ccmp::key_pn(f)?;
        let (key, last) = match idx {
            0 => (self.tk, &mut self.rx_pn),
            i => {
                let (key, last) = self.groups[usize::from(i)].as_mut()?;
                (*key, last)
            }
        };
        let (clear, pn) = ccmp::decrypt(&key, f)?;
        if pn <= *last {
            return None;
        }
        *last = pn;
        Some(clear)
    }

    /// An EAPOL frame the access point sent over the link: a group key
    /// handshake, answered (encrypted) and its key installed.
    pub fn eapol(&mut self, radio: &mut dyn Radio, bssid: [u8; 6], pdu: &[u8]) {
        if let Ok(Step::Installed(m)) = self.supplicant.input(pdu) {
            if let Some(k) = self.supplicant.keys() {
                let (idx, key, rsc) = k.gtk;
                self.groups[usize::from(idx)] = Some((key, rsc));
            }
            send_eapol(radio, bssid, &m, Some(self));
        }
    }
}

/// The EAPOL PDU data frame `f` (in the clear) from `bssid` to `me`
/// carries, if it is one.
pub fn eapol_of(f: &[u8], me: [u8; 6], bssid: [u8; 6]) -> Option<Vec<u8>> {
    let eth = frame::data_from_ds(f, me, bssid)?;
    (eth[12..14] == eapol::ETHERTYPE.to_be_bytes() && eth[..6] == me).then(|| eth[14..].to_vec())
}

/// Send EAPOL PDU `pdu` to the access point, encrypted when `rsn` is.
fn send_eapol(radio: &mut dyn Radio, bssid: [u8; 6], pdu: &[u8], rsn: Option<&mut Rsn>) {
    let me = radio.mac();
    let mut eth = Vec::with_capacity(14 + pdu.len());
    eth.extend_from_slice(&bssid);
    eth.extend_from_slice(&me);
    eth.extend_from_slice(&eapol::ETHERTYPE.to_be_bytes());
    eth.extend_from_slice(pdu);
    let Some(f) = frame::data_to_ds(me, bssid, &eth) else {
        return;
    };
    match rsn {
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

/// 32 bytes from the kernel's CSPRNG: the SNonce.
fn random() -> Option<[u8; 32]> {
    let fd = open_flags(b"/dev/urandom", O_RDONLY)?;
    let mut out = [0u8; 32];
    let n = read(fd, &mut out);
    close(fd);
    (n == out.len()).then_some(out)
}
