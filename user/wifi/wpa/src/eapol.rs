//! EAPOL-Key frames (IEEE 802.11-2020 12.7.2) and the supplicant's side of
//! the 4-way handshake and of the group key handshake (12.7.6, 12.7.7):
//! the access point's messages in, the station's answers out, the keys
//! once agreed.
//!
//! A frame here is the EAPOL PDU an 802.11 data frame carries after its
//! LLC/SNAP header and EtherType [`ETHERTYPE`]: version, type 3 (key),
//! body length, then the key descriptor.

use alloc::vec::Vec;

use crate::{hmac_sha1, Ptk};

/// EtherType 0x888e: 802.1X (EAPOL).
pub const ETHERTYPE: u16 = 0x888e;

/// The EAPOL header: version, packet type, body length.
const HDR: usize = 4;
const TYPE_KEY: u8 = 3;
/// Descriptor type 2: the RSN key descriptor.
const DESC_RSN: u8 = 2;
/// The descriptor's fields up to its key data, after the EAPOL header:
/// type, information, key length, replay counter, nonce, IV, RSC,
/// reserved, MIC, key data length.
const DESC: usize = 1 + 2 + 2 + 8 + 32 + 16 + 8 + 8 + 16 + 2;
const OFF_INFO: usize = HDR + 1;
const OFF_REPLAY: usize = HDR + 5;
const OFF_NONCE: usize = HDR + 13;
const OFF_RSC: usize = HDR + 61;
const OFF_MIC: usize = HDR + 77;
const OFF_DATALEN: usize = HDR + 93;

/// Key information bits.
pub const INFO_VERSION_AES: u16 = 2; // HMAC-SHA1-128 MIC, AES key wrap
pub const INFO_PAIRWISE: u16 = 1 << 3;
pub const INFO_INSTALL: u16 = 1 << 6;
pub const INFO_ACK: u16 = 1 << 7;
pub const INFO_MIC: u16 = 1 << 8;
pub const INFO_SECURE: u16 = 1 << 9;
pub const INFO_ENCRYPTED: u16 = 1 << 12;

/// The GTK key data encapsulation's OUI and type (00-0f-ac:1).
const KDE_GTK: [u8; 4] = [0x00, 0x0f, 0xac, 1];

/// An EAPOL-Key frame's fields.
pub struct Key<'a> {
    pub info: u16,
    pub replay: u64,
    pub nonce: &'a [u8; 32],
    pub rsc: u64,
    pub data: &'a [u8],
}

/// The fields of EAPOL-Key frame `f`, when it is one with an RSN
/// descriptor.
pub fn parse(f: &[u8]) -> Option<Key<'_>> {
    if f.len() < HDR + DESC || f[1] != TYPE_KEY || f[HDR] != DESC_RSN {
        return None;
    }
    let body = usize::from(u16::from_be_bytes([f[2], f[3]]));
    let n = usize::from(u16::from_be_bytes([f[OFF_DATALEN], f[OFF_DATALEN + 1]]));
    if body < DESC + n || f.len() < HDR + DESC + n {
        return None;
    }
    Some(Key {
        info: u16::from_be_bytes([f[OFF_INFO], f[OFF_INFO + 1]]),
        replay: u64::from_be_bytes(f[OFF_REPLAY..OFF_REPLAY + 8].try_into().ok()?),
        nonce: f[OFF_NONCE..OFF_NONCE + 32].try_into().ok()?,
        rsc: u64::from_le_bytes(f[OFF_RSC..OFF_RSC + 8].try_into().ok()?),
        data: &f[HDR + DESC..HDR + DESC + n],
    })
}

/// An EAPOL-Key frame with these fields: its nonce zero when it carries
/// none (messages 4 and the group key handshake's), its MIC made with
/// `kck` when it has one (all but message 1; `info` says [`INFO_MIC`]).
pub fn build(
    info: u16,
    key_len: u16,
    replay: u64,
    nonce: Option<&[u8; 32]>,
    rsc: u64,
    data: &[u8],
    kck: Option<&[u8; 16]>,
) -> Vec<u8> {
    debug_assert_eq!(info & INFO_MIC != 0, kck.is_some());
    let mut f = Vec::with_capacity(HDR + DESC + data.len());
    f.extend_from_slice(&[2, TYPE_KEY]);
    f.extend_from_slice(&((DESC + data.len()) as u16).to_be_bytes());
    f.push(DESC_RSN);
    f.extend_from_slice(&info.to_be_bytes());
    f.extend_from_slice(&key_len.to_be_bytes());
    f.extend_from_slice(&replay.to_be_bytes());
    f.extend_from_slice(nonce.unwrap_or(&[0; 32]));
    f.extend_from_slice(&[0; 16]); // IV
    f.extend_from_slice(&rsc.to_le_bytes());
    f.extend_from_slice(&[0; 8]); // reserved
    f.extend_from_slice(&[0; 16]); // MIC, below
    f.extend_from_slice(&(data.len() as u16).to_be_bytes());
    f.extend_from_slice(data);
    if let Some(kck) = kck {
        let m = mic(kck, &f);
        f[OFF_MIC..OFF_MIC + 16].copy_from_slice(&m);
    }
    f
}

/// The MIC of frame `f` (its MIC field taken as zero): HMAC-SHA1-128.
pub fn mic(kck: &[u8; 16], f: &[u8]) -> [u8; 16] {
    let h = hmac_sha1(kck, &[&f[..OFF_MIC], &[0; 16], &f[OFF_MIC + 16..]]);
    h[..16].try_into().unwrap()
}

/// Whether frame `f`'s MIC is `kck`'s.
pub fn mic_ok(kck: &[u8; 16], f: &[u8]) -> bool {
    // Not secret-dependent timing worth guarding against here: a forger
    // learns nothing from how many bytes of a wrong MIC matched.
    f.len() >= OFF_MIC + 16 && mic(kck, f) == f[OFF_MIC..OFF_MIC + 16]
}

/// Key data encrypted with the KEK: AES key wrap (RFC 3394), padded first
/// to a multiple of 8 bytes, at least 16, with 0xdd then zeros (12.7.2).
pub fn wrap(kek: &[u8; 16], data: &[u8]) -> Vec<u8> {
    let mut p = data.to_vec();
    if p.len() < 16 || p.len() % 8 != 0 {
        p.push(0xdd);
        while p.len() < 16 || p.len() % 8 != 0 {
            p.push(0);
        }
    }
    let mut out = alloc::vec![0u8; p.len() + 8];
    aes_kw::KekAes128::from(*kek).wrap(&p, &mut out).expect("a multiple of 8 bytes, at least 16");
    out
}

/// Key data decrypted with the KEK; `None` when its integrity check fails.
pub fn unwrap(kek: &[u8; 16], data: &[u8]) -> Option<Vec<u8>> {
    if data.len() < 24 || data.len() % 8 != 0 {
        return None;
    }
    let mut out = alloc::vec![0u8; data.len() - 8];
    aes_kw::KekAes128::from(*kek).unwrap(data, &mut out).ok()?;
    Some(out)
}

/// The group key a GTK KDE in key data `data` carries: its index (1 to 3)
/// and the key. The data is elements and KDEs, then perhaps the padding.
pub fn gtk(data: &[u8]) -> Option<(u8, [u8; 16])> {
    let mut d = data;
    while d.len() >= 2 && d[0] != 0 {
        let len = usize::from(d[1]);
        let body = d.get(2..2 + len)?;
        if d[0] == 0xdd && len >= 6 + 16 && body[..4] == KDE_GTK {
            let idx = body[4] & 3;
            return (idx != 0).then(|| (idx, body[6..22].try_into().unwrap()));
        }
        d = &d[2 + len..];
    }
    None
}

/// The first element `id` (with its id and length) in key data `data`.
fn element(data: &[u8], id: u8) -> Option<&[u8]> {
    let mut d = data;
    while d.len() >= 2 && d[0] != 0 {
        let len = usize::from(d[1]);
        let whole = d.get(..2 + len)?;
        if d[0] == id {
            return Some(whole);
        }
        d = &d[2 + len..];
    }
    None
}

/// Why a handshake failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    /// A frame's MIC was not the PTK's: almost always a wrong passphrase
    /// (message 3), or a forged frame.
    Mic,
    /// The access point's RSN element in message 3 is not its beacon's (a
    /// downgrade attempt, 12.7.6.4).
    Rsn,
    /// Message 3 carried no GTK, or its key data did not decrypt.
    KeyData,
}

/// What a frame from the access point asks of the station.
pub enum Step {
    /// Not for the handshake, a replay, or out of order: ignored.
    Ignore,
    /// Send this frame (message 2).
    Reply(Vec<u8>),
    /// Send this frame (message 4, or the group key handshake's message
    /// 2); the keys are installed: [`Supplicant::keys`].
    Installed(Vec<u8>),
}

/// The keys the handshakes agreed on.
#[derive(Clone)]
pub struct Keys {
    pub tk: [u8; 16],
    /// The group key: its index, the key, and the packet number its frames
    /// start after (the access point's RSC).
    pub gtk: (u8, [u8; 16], u64),
}

/// The station's side of the handshakes with access point `aa`.
pub struct Supplicant {
    pmk: [u8; 32],
    aa: [u8; 6],
    spa: [u8; 6],
    /// The RSN element the station sent in its association request, and
    /// the one the access point's beacon had.
    own_rsn: Vec<u8>,
    ap_rsn: Vec<u8>,
    snonce: [u8; 32],
    /// The PTK of message 1's ANonce, before message 3 confirms it.
    ptk: Option<Ptk>,
    /// The last replay counter of a frame the station answered.
    replay: Option<u64>,
    keys: Option<Keys>,
    kck: [u8; 16],
}

impl Supplicant {
    /// `snonce` comes from a good random source; the RSN elements are whole
    /// (id and length first).
    pub fn new(pmk: [u8; 32], aa: [u8; 6], spa: [u8; 6], own_rsn: &[u8], ap_rsn: &[u8], snonce: [u8; 32]) -> Self {
        Supplicant {
            pmk,
            aa,
            spa,
            own_rsn: own_rsn.to_vec(),
            ap_rsn: ap_rsn.to_vec(),
            snonce,
            ptk: None,
            replay: None,
            keys: None,
            kck: [0; 16],
        }
    }

    pub fn keys(&self) -> Option<&Keys> {
        self.keys.as_ref()
    }

    /// Frame `f` (an EAPOL PDU) from the access point.
    pub fn input(&mut self, f: &[u8]) -> Result<Step, Error> {
        let Some(k) = parse(f) else {
            return Ok(Step::Ignore);
        };
        if k.info & 7 != INFO_VERSION_AES || k.info & INFO_ACK == 0 || self.replay.is_some_and(|r| k.replay <= r) {
            return Ok(Step::Ignore);
        }
        let pairwise = k.info & INFO_PAIRWISE != 0;
        match (pairwise, k.info & INFO_MIC != 0) {
            // Message 1: the ANonce. A new one (a restarted handshake)
            // makes a new PTK.
            (true, false) => {
                let ptk = Ptk::derive(&self.pmk, self.aa, self.spa, k.nonce, &self.snonce);
                let info = INFO_VERSION_AES | INFO_PAIRWISE | INFO_MIC;
                let reply = build(info, 0, k.replay, Some(&self.snonce), 0, &self.own_rsn, Some(&ptk.kck));
                self.ptk = Some(ptk);
                self.replay = Some(k.replay);
                Ok(Step::Reply(reply))
            }
            // Message 3: the access point has the PTK too; the GTK.
            (true, true) => {
                let Some(ptk) = self.ptk.clone() else {
                    return Ok(Step::Ignore);
                };
                if !mic_ok(&ptk.kck, f) {
                    return Err(Error::Mic);
                }
                if k.info & (INFO_INSTALL | INFO_ENCRYPTED) != INFO_INSTALL | INFO_ENCRYPTED {
                    return Err(Error::KeyData);
                }
                let data = unwrap(&ptk.kek, k.data).ok_or(Error::KeyData)?;
                if element(&data, 48) != Some(&self.ap_rsn[..]) {
                    return Err(Error::Rsn);
                }
                let (idx, gtk) = gtk(&data).ok_or(Error::KeyData)?;
                self.replay = Some(k.replay);
                let info = INFO_VERSION_AES | INFO_PAIRWISE | INFO_MIC | INFO_SECURE;
                let reply = build(info, 0, k.replay, None, 0, &[], Some(&ptk.kck));
                self.kck = ptk.kck;
                self.keys = Some(Keys { tk: ptk.tk, gtk: (idx, gtk, k.rsc) });
                Ok(Step::Installed(reply))
            }
            // The group key handshake's message 1: a new GTK.
            (false, true) => {
                let Some(ptk) = self.ptk.as_ref().filter(|_| self.keys.is_some()) else {
                    return Ok(Step::Ignore);
                };
                if !mic_ok(&ptk.kck, f) {
                    return Err(Error::Mic);
                }
                let data = unwrap(&ptk.kek, k.data).ok_or(Error::KeyData)?;
                let (idx, gtk) = gtk(&data).ok_or(Error::KeyData)?;
                self.replay = Some(k.replay);
                let info = INFO_VERSION_AES | INFO_MIC | INFO_SECURE;
                let reply = build(info, 0, k.replay, None, 0, &[], Some(&self.kck));
                if let Some(keys) = self.keys.as_mut() {
                    keys.gtk = (idx, gtk, k.rsc);
                }
                Ok(Step::Installed(reply))
            }
            (false, false) => Ok(Step::Ignore),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{psk, RSN_PSK_CCMP};

    /// RFC 3394 4.1: a 128-bit key wrapped with a 128-bit KEK.
    #[test]
    fn key_wrap_vector() {
        let kek: [u8; 16] = core::array::from_fn(|i| i as u8);
        let key: [u8; 16] = core::array::from_fn(|i| (i as u8) * 0x11);
        let want = [
            0x1f, 0xa6, 0x8b, 0x0a, 0x81, 0x12, 0xb4, 0x47, 0xae, 0xf3, 0x4b, 0xd8, 0xfb, 0x5a, 0x7b, 0x82, 0x9d, 0x3e,
            0x86, 0x23, 0x71, 0xd2, 0xcf, 0xe5,
        ];
        assert_eq!(wrap(&kek, &key), want);
        assert_eq!(unwrap(&kek, &want).unwrap(), key);
        let mut bad = want;
        bad[3] ^= 1;
        assert!(unwrap(&kek, &bad).is_none());
    }

    /// A message 2's MIC against Python's hmac.
    #[test]
    fn mic_vector() {
        let kck = [0x5a; 16];
        let info = INFO_VERSION_AES | INFO_PAIRWISE | INFO_MIC;
        let f = build(info, 0, 1, Some(&[0x22; 32]), 0, &RSN_PSK_CCMP, Some(&kck));
        let want = "2227d1a9c1e7ac8958da12748cd4cb72";
        let want: Vec<u8> = (0..32).step_by(2).map(|i| u8::from_str_radix(&want[i..i + 2], 16).unwrap()).collect();
        assert_eq!(f[OFF_MIC..OFF_MIC + 16], want[..]);
        assert!(mic_ok(&kck, &f));
        assert!(!mic_ok(&[0; 16], &f));
    }

    const AA: [u8; 6] = [0x02, 0x00, 0x5e, 0x00, 0x01, 0x06];
    const SPA: [u8; 6] = [0x52, 0x54, 0x00, 0x12, 0x34, 0x57];

    fn gtk_kde(idx: u8, gtk: &[u8; 16]) -> Vec<u8> {
        let mut d = alloc::vec![0xdd, 22, 0x00, 0x0f, 0xac, 1, idx, 0];
        d.extend_from_slice(gtk);
        d
    }

    /// The access point's side of a handshake, as a test: messages 1 and
    /// 3, then a group key handshake; the supplicant's answers and keys.
    #[test]
    fn handshakes() {
        let pmk = psk(b"myos-passphrase", b"myos-wpa2").unwrap();
        let mut s = Supplicant::new(pmk, AA, SPA, &RSN_PSK_CCMP, &RSN_PSK_CCMP, [0x22; 32]);
        let anonce = [0x11; 32];
        let m1 = build(INFO_VERSION_AES | INFO_PAIRWISE | INFO_ACK, 16, 1, Some(&anonce), 0, &[], None);
        let Ok(Step::Reply(m2)) = s.input(&m1) else { panic!("no message 2") };
        let k2 = parse(&m2).unwrap();
        let ptk = Ptk::derive(&pmk, AA, SPA, &anonce, k2.nonce);
        assert!(mic_ok(&ptk.kck, &m2));
        assert_eq!(k2.data, RSN_PSK_CCMP);
        // A replay of message 1 is ignored.
        assert!(matches!(s.input(&m1), Ok(Step::Ignore)));

        let mut data = RSN_PSK_CCMP.to_vec();
        data.extend_from_slice(&gtk_kde(1, &[0x33; 16]));
        let info = INFO_VERSION_AES | INFO_PAIRWISE | INFO_ACK | INFO_MIC | INFO_INSTALL | INFO_SECURE | INFO_ENCRYPTED;
        let m3 = build(info, 16, 2, Some(&anonce), 7, &wrap(&ptk.kek, &data), Some(&ptk.kck));
        let Ok(Step::Installed(m4)) = s.input(&m3) else { panic!("no message 4") };
        assert!(mic_ok(&ptk.kck, &m4));
        assert_eq!(parse(&m4).unwrap().replay, 2);
        let keys = s.keys().unwrap();
        assert_eq!(keys.tk, ptk.tk);
        assert_eq!(keys.gtk, (1, [0x33; 16], 7));

        let info = INFO_VERSION_AES | INFO_ACK | INFO_MIC | INFO_SECURE | INFO_ENCRYPTED;
        let g1 = build(info, 0, 3, None, 0, &wrap(&ptk.kek, &gtk_kde(2, &[0x44; 16])), Some(&ptk.kck));
        let Ok(Step::Installed(g2)) = s.input(&g1) else { panic!("no group message 2") };
        assert!(mic_ok(&ptk.kck, &g2));
        assert_eq!(s.keys().unwrap().gtk, (2, [0x44; 16], 0));
    }

    /// A wrong passphrase: message 3's MIC is not the station's.
    #[test]
    fn wrong_passphrase() {
        let pmk = psk(b"not-the-passphrase", b"myos-wpa2").unwrap();
        let mut s = Supplicant::new(pmk, AA, SPA, &RSN_PSK_CCMP, &RSN_PSK_CCMP, [0x22; 32]);
        let ap = Ptk::derive(&psk(b"myos-passphrase", b"myos-wpa2").unwrap(), AA, SPA, &[0x11; 32], &[0x22; 32]);
        let m1 = build(INFO_VERSION_AES | INFO_PAIRWISE | INFO_ACK, 16, 1, Some(&[0x11; 32]), 0, &[], None);
        assert!(matches!(s.input(&m1), Ok(Step::Reply(_))));
        let info = INFO_VERSION_AES | INFO_PAIRWISE | INFO_ACK | INFO_MIC | INFO_INSTALL | INFO_SECURE | INFO_ENCRYPTED;
        let m3 = build(info, 16, 2, Some(&[0x11; 32]), 0, &wrap(&ap.kek, &RSN_PSK_CCMP), Some(&ap.kck));
        assert_eq!(s.input(&m3).err(), Some(Error::Mic));
        assert!(s.keys().is_none());
    }

    /// Message 3 naming other ciphers than the beacon did is refused.
    #[test]
    fn rsn_mismatch() {
        let pmk = psk(b"myos-passphrase", b"myos-wpa2").unwrap();
        let mut s = Supplicant::new(pmk, AA, SPA, &RSN_PSK_CCMP, &RSN_PSK_CCMP, [0x22; 32]);
        let m1 = build(INFO_VERSION_AES | INFO_PAIRWISE | INFO_ACK, 16, 1, Some(&[0x11; 32]), 0, &[], None);
        assert!(matches!(s.input(&m1), Ok(Step::Reply(_))));
        let ptk = Ptk::derive(&pmk, AA, SPA, &[0x11; 32], &[0x22; 32]);
        let mut rsn = RSN_PSK_CCMP;
        rsn[7] = 2; // TKIP group cipher
        let mut data = rsn.to_vec();
        data.extend_from_slice(&gtk_kde(1, &[0x33; 16]));
        let info = INFO_VERSION_AES | INFO_PAIRWISE | INFO_ACK | INFO_MIC | INFO_INSTALL | INFO_SECURE | INFO_ENCRYPTED;
        let m3 = build(info, 16, 2, Some(&[0x11; 32]), 0, &wrap(&ptk.kek, &data), Some(&ptk.kck));
        assert_eq!(s.input(&m3).err(), Some(Error::Rsn));
    }
}
