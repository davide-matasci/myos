//! WPA2-PSK for the wifi daemon (issue #495, `docs/wifi.md`): the keys a
//! passphrase and the 4-way handshake make (IEEE 802.11-2020 12.7), the
//! EAPOL-Key frames of the supplicant's side of that handshake and of the
//! group key handshake (`eapol.rs`), and CCMP, the data frames' encryption
//! (`ccmp.rs`). The cryptography is RustCrypto's; `no_std`, so the daemon
//! links it and `cargo test -p wpa` checks it on the host.

#![no_std]

extern crate alloc;

pub mod ccmp;
pub mod eapol;

use hmac::{Hmac, Mac};
use sha1::Sha1;

/// The pairwise master key of a network: from its passphrase (8 to 63
/// printable ASCII characters) and SSID, PBKDF2-HMAC-SHA1 with 4096
/// iterations; or the key itself, given as 64 hex digits.
pub fn psk(passphrase: &[u8], ssid: &[u8]) -> Option<[u8; 32]> {
    let mut pmk = [0u8; 32];
    if passphrase.len() == 64 {
        for (i, b) in pmk.iter_mut().enumerate() {
            let hex = core::str::from_utf8(&passphrase[2 * i..2 * i + 2]).ok()?;
            *b = u8::from_str_radix(hex, 16).ok()?;
        }
        return Some(pmk);
    }
    if !(8..=63).contains(&passphrase.len()) || !passphrase.iter().all(|c| (0x20..0x7f).contains(c)) {
        return None;
    }
    pbkdf2::pbkdf2_hmac::<Sha1>(passphrase, ssid, 4096, &mut pmk);
    Some(pmk)
}

/// HMAC-SHA1 of `parts`, one after the other.
pub fn hmac_sha1(key: &[u8], parts: &[&[u8]]) -> [u8; 20] {
    let mut m = <Hmac<Sha1> as Mac>::new_from_slice(key).expect("HMAC takes any key length");
    for p in parts {
        m.update(p);
    }
    m.finalize().into_bytes().into()
}

/// The 802.11 PRF (12.7.1.2): `out.len()` bytes of HMAC-SHA1(key, label,
/// 0, data, i) for i = 0, 1, ...
pub fn prf(key: &[u8], label: &[u8], data: &[u8], out: &mut [u8]) {
    for (i, chunk) in out.chunks_mut(20).enumerate() {
        let h = hmac_sha1(key, &[label, &[0], data, &[i as u8]]);
        chunk.copy_from_slice(&h[..chunk.len()]);
    }
}

/// The pairwise transient key of CCMP with PSK: the EAPOL-Key frames' MIC
/// key and key encryption key, and the temporal key of the data frames.
#[derive(Clone)]
pub struct Ptk {
    pub kck: [u8; 16],
    pub kek: [u8; 16],
    pub tk: [u8; 16],
}

impl Ptk {
    /// The key of the authenticator `aa` and supplicant `spa` with these
    /// nonces (12.7.1.3: PRF-384 over the smaller then the larger address
    /// and nonce).
    pub fn derive(pmk: &[u8; 32], aa: [u8; 6], spa: [u8; 6], anonce: &[u8; 32], snonce: &[u8; 32]) -> Ptk {
        let mut data = [0u8; 6 + 6 + 32 + 32];
        let (lo, hi) = if aa < spa { (aa, spa) } else { (spa, aa) };
        data[..6].copy_from_slice(&lo);
        data[6..12].copy_from_slice(&hi);
        let (lo, hi) = if anonce < snonce { (anonce, snonce) } else { (snonce, anonce) };
        data[12..44].copy_from_slice(lo);
        data[44..].copy_from_slice(hi);
        let mut k = [0u8; 48];
        prf(pmk, b"Pairwise key expansion", &data, &mut k);
        let mut ptk = Ptk { kck: [0; 16], kek: [0; 16], tk: [0; 16] };
        ptk.kck.copy_from_slice(&k[..16]);
        ptk.kek.copy_from_slice(&k[16..32]);
        ptk.tk.copy_from_slice(&k[32..]);
        ptk
    }
}

/// The RSN element a PSK station with CCMP sends in its association
/// request and its handshake: version 1, CCMP group and pairwise ciphers,
/// PSK key management, no capabilities.
pub const RSN_PSK_CCMP: [u8; 22] = [
    48, 20, 1, 0, 0x00, 0x0f, 0xac, 4, 1, 0, 0x00, 0x0f, 0xac, 4, 1, 0, 0x00, 0x0f, 0xac, 2, 0, 0,
];

#[cfg(test)]
mod tests {
    use super::*;

    fn unhex(s: &str) -> alloc::vec::Vec<u8> {
        let s: alloc::string::String = s.chars().filter(|c| !c.is_whitespace()).collect();
        (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap()).collect()
    }

    /// IEEE 802.11-2020 J.4.2, the PSK of a passphrase and SSID.
    #[test]
    fn psk_vectors() {
        assert_eq!(
            psk(b"password", b"IEEE").unwrap().to_vec(),
            unhex("f42c6fc52df0ebef9ebb4b90b38a5f902e83fe1b135a70e23aed762e9710a12e")
        );
        assert_eq!(
            psk(b"ThisIsAPassword", b"ThisIsASSID").unwrap().to_vec(),
            unhex("0dc0d6eb90555ed6419756b9a15ec3e3209b63df707dd508d14581f8982721af")
        );
        let hex = "0dc0d6eb90555ed6419756b9a15ec3e3209b63df707dd508d14581f8982721af";
        assert_eq!(psk(hex.as_bytes(), b"anything").unwrap().to_vec(), unhex(hex));
        assert!(psk(b"short", b"IEEE").is_none());
        assert!(psk(b"tab\tinside", b"IEEE").is_none());
    }

    /// The PRF and the PTK, against Python's hmac (an independent
    /// implementation).
    #[test]
    fn ptk_vector() {
        let pmk: [u8; 32] = psk(b"password", b"IEEE").unwrap();
        let aa = [0x02, 0x00, 0x5e, 0x00, 0x01, 0x06];
        let spa = [0x52, 0x54, 0x00, 0x12, 0x34, 0x57];
        let anonce = [0x11; 32];
        let snonce = [0x22; 32];
        let ptk = Ptk::derive(&pmk, aa, spa, &anonce, &snonce);
        // The order of the addresses and nonces does not matter.
        let other = Ptk::derive(&pmk, spa, aa, &snonce, &anonce);
        assert_eq!(ptk.tk, other.tk);
        let mut all = alloc::vec::Vec::new();
        all.extend_from_slice(&ptk.kck);
        all.extend_from_slice(&ptk.kek);
        all.extend_from_slice(&ptk.tk);
        let want = "fa3095876a03b68d392bd0b425daf37ca95ead865bca158389eab3f1732d28d3
                    e468eeff37fa3494a36ac74c4a11ec59";
        assert_eq!(all, unhex(want));
    }
}
