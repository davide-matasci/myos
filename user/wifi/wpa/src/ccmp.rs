//! CCMP (IEEE 802.11-2020 12.5.3): a data frame's body encrypted with
//! AES-CCM under the temporal key, an 8-byte MIC appended, the CCMP header
//! (packet number and key index) between the 802.11 header and the body.

use alloc::vec::Vec;

use aes::Aes128;
use ccm::aead::generic_array::GenericArray;
use ccm::aead::{AeadInPlace, KeyInit};
use ccm::consts::{U13, U8};
use ccm::Ccm;

type AesCcm = Ccm<Aes128, U8, U13>;

/// The CCMP header and the MIC.
pub const HDR: usize = 8;
pub const MIC: usize = 8;

/// Frame control byte 1: protected frame.
const PROTECTED: u8 = 0x40;
/// The CCMP header's key id byte: extended IV, and the key index above.
const EXT_IV: u8 = 0x20;

/// The length of a data frame's 802.11 header: three addresses, a fourth
/// when it goes between distribution systems, QoS control when a QoS data
/// frame.
pub fn header_len(f: &[u8]) -> Option<usize> {
    let fc = f.get(..2)?;
    let mut n = 24;
    if fc[1] & 3 == 3 {
        n += 6;
    }
    if fc[0] & 0x88 == 0x88 {
        n += 2;
    }
    (f.len() >= n).then_some(n)
}

/// The nonce and additional authenticated data of frame `f` (header
/// `hl` bytes) with packet number `pn` (12.5.3.3.3, 12.5.3.3.4).
fn nonce_aad(f: &[u8], hl: usize, pn: u64) -> ([u8; 13], Vec<u8>) {
    let qos = f[0] & 0x88 == 0x88;
    let mut nonce = [0u8; 13];
    nonce[0] = if qos { f[hl - 2] & 0x0f } else { 0 };
    nonce[1..7].copy_from_slice(&f[10..16]);
    nonce[7..].copy_from_slice(&pn.to_be_bytes()[2..]);
    let mut aad = Vec::with_capacity(30);
    // Frame control: the subtype's bits 4 to 6 masked; retry, power
    // management and more data cleared; protected set; order cleared on a
    // QoS frame.
    let mut fc1 = (f[1] & !0x38) | PROTECTED;
    if qos {
        fc1 &= !0x80;
    }
    aad.extend_from_slice(&[f[0] & 0x8f, fc1]);
    aad.extend_from_slice(&f[4..22]);
    // Sequence control: the fragment number only.
    aad.extend_from_slice(&[f[22] & 0x0f, 0]);
    if f[1] & 3 == 3 {
        aad.extend_from_slice(&f[24..30]);
    }
    if qos {
        aad.extend_from_slice(&[f[hl - 2] & 0x0f, 0]);
    }
    (nonce, aad)
}

/// Data frame `f` (in the clear) encrypted with temporal key `tk` as
/// packet `pn` of key index `key_id`.
pub fn encrypt(tk: &[u8; 16], pn: u64, key_id: u8, f: &[u8]) -> Option<Vec<u8>> {
    let hl = header_len(f)?;
    let (nonce, aad) = nonce_aad(f, hl, pn);
    let p = pn.to_le_bytes();
    let mut out = Vec::with_capacity(f.len() + HDR + MIC);
    out.extend_from_slice(&f[..hl]);
    out[1] |= PROTECTED;
    out.extend_from_slice(&[p[0], p[1], 0, EXT_IV | key_id << 6, p[2], p[3], p[4], p[5]]);
    let mut body = f[hl..].to_vec();
    let tag = AesCcm::new(GenericArray::from_slice(tk))
        .encrypt_in_place_detached(GenericArray::from_slice(&nonce), &aad, &mut body)
        .ok()?;
    out.extend_from_slice(&body);
    out.extend_from_slice(&tag);
    Some(out)
}

/// The key index and packet number of protected frame `f`.
pub fn key_pn(f: &[u8]) -> Option<(u8, u64)> {
    let hl = header_len(f)?;
    let h = f.get(hl..hl + HDR)?;
    if f[1] & PROTECTED == 0 || h[3] & EXT_IV == 0 {
        return None;
    }
    Some((h[3] >> 6, u64::from_le_bytes([h[0], h[1], h[4], h[5], h[6], h[7], 0, 0])))
}

/// Protected frame `f` decrypted with `tk`: the frame in the clear (the
/// protected bit cleared) and its packet number; `None` when its MIC is
/// not `tk`'s.
pub fn decrypt(tk: &[u8; 16], f: &[u8]) -> Option<(Vec<u8>, u64)> {
    let hl = header_len(f)?;
    let (_, pn) = key_pn(f)?;
    if f.len() < hl + HDR + MIC {
        return None;
    }
    let (nonce, aad) = nonce_aad(f, hl, pn);
    let (body, tag) = f[hl + HDR..].split_at(f.len() - hl - HDR - MIC);
    let mut body = body.to_vec();
    AesCcm::new(GenericArray::from_slice(tk))
        .decrypt_in_place_detached(GenericArray::from_slice(&nonce), &aad, &mut body, GenericArray::from_slice(tag))
        .ok()?;
    let mut out = Vec::with_capacity(hl + body.len());
    out.extend_from_slice(&f[..hl]);
    out[1] &= !PROTECTED;
    out.extend_from_slice(&body);
    Some((out, pn))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unhex(s: &str) -> Vec<u8> {
        let s: alloc::string::String = s.chars().filter(|c| !c.is_whitespace()).collect();
        (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap()).collect()
    }

    /// A data frame to the distribution system and a QoS one, against
    /// Python's `cryptography` AES-CCM with the nonce and AAD built by
    /// `vectors.py` (an independent implementation of 12.5.3.3).
    #[test]
    fn vectors() {
        let tk: [u8; 16] = core::array::from_fn(|i| 0xc0 + i as u8);
        let cases = [
            ("data", include_str!("../vectors/ccmp-data.hex")),
            ("qos", include_str!("../vectors/ccmp-qos.hex")),
        ];
        for (name, want) in cases {
            let mut f = unhex("0801 0000 02005e000106 525400123457 ffffffffffff 3012");
            if name == "qos" {
                f[0] = 0x88;
                f[1] |= 0x08; // retry: masked out of the AAD
                f.extend_from_slice(&[0x05, 0x00]);
            }
            f.extend_from_slice(b"\xaa\xaa\x03\x00\x00\x00\x08\x00hello, wifi");
            let enc = encrypt(&tk, 0x0102_0304_0506, 0, &f).unwrap();
            assert_eq!(enc, unhex(want), "{name}");
            assert_eq!(key_pn(&enc), Some((0, 0x0102_0304_0506)));
            let (dec, pn) = decrypt(&tk, &enc).unwrap();
            assert_eq!((dec, pn), (f.clone(), 0x0102_0304_0506));
            let mut bad = enc.clone();
            *bad.last_mut().unwrap() ^= 1;
            assert!(decrypt(&tk, &bad).is_none());
        }
    }
}
