//! The firmware files: the image's reset vector, the NVRAM text in the form
//! the chip reads it, and the CLM blob in the pieces the firmware takes.

use alloc::vec::Vec;

use crate::reg::*;

/// The firmware image's first word: where the CPU starts.
pub fn reset_vector(firmware: &[u8]) -> u32 {
    match firmware {
        [a, b, c, d, ..] => u32::from_le_bytes([*a, *b, *c, *d]),
        _ => 0,
    }
}

/// The NVRAM for the chip's RAM, from the board's `.txt`: `name=value`
/// lines NUL-separated, comments and blank lines dropped, padded to a
/// word, then the token (the length in words and its complement) the
/// firmware checks. `macaddr`: the station's address appended when the
/// file does not give one.
pub fn nvram(text: &[u8], macaddr: Option<[u8; 6]>) -> Vec<u8> {
    let mut out = Vec::with_capacity(text.len() + 64);
    let mut count = 0;
    let mut skip = false;
    for &c in text {
        match c {
            b'\n' => {
                if count > 0 {
                    out.push(0);
                }
                count = 0;
                skip = false;
            }
            _ if skip => {}
            b'#' if count == 0 => skip = true,
            b'\r' => {}
            _ => {
                out.push(c);
                count += 1;
            }
        }
    }
    if count > 0 {
        out.push(0);
    }
    if let Some(mac) = macaddr {
        let mut line = Vec::from(b"macaddr=" as &[u8]);
        for (i, b) in mac.iter().enumerate() {
            if i != 0 {
                line.push(b':');
            }
            line.extend_from_slice(&[hex(b >> 4), hex(b & 15)]);
        }
        out.extend_from_slice(&line);
        out.push(0);
    }
    // Room for the end marker, then a word boundary.
    let pad = (out.len() + 1).next_multiple_of(4) - out.len();
    out.extend(core::iter::repeat_n(0u8, pad));
    let words = (out.len() / 4) as u32 & 0xffff;
    let token = words | (!words & 0xffff) << 16;
    out.extend_from_slice(&token.to_le_bytes());
    out
}

fn hex(v: u8) -> u8 {
    b"0123456789abcdef"[usize::from(v)]
}

/// The pieces a blob (the CLM) is handed to the firmware in, each a
/// `dload_data` record for the `clmload` variable.
pub fn dload(blob: &[u8]) -> Vec<Vec<u8>> {
    let mut out = Vec::new();
    let n = blob.len().div_ceil(DLOAD_MAX_LEN).max(1);
    for (i, piece) in blob.chunks(DLOAD_MAX_LEN).enumerate() {
        let mut flag = DLOAD_FLAG_HANDLER_VER_1;
        if i == 0 {
            flag |= DLOAD_FLAG_BEGIN;
        }
        if i + 1 == n {
            flag |= DLOAD_FLAG_END;
        }
        let mut rec = Vec::with_capacity(DLOAD_HDR_LEN + piece.len());
        rec.extend_from_slice(&flag.to_le_bytes());
        rec.extend_from_slice(&DLOAD_TYPE_CLM.to_le_bytes());
        rec.extend_from_slice(&(piece.len() as u32).to_le_bytes());
        rec.extend_from_slice(&0u32.to_le_bytes());
        rec.extend_from_slice(piece);
        out.push(rec);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nvram_text() {
        let v = nvram(b"# the board\r\nmanfid=0x2d0\n\nsromrev=11\n", None);
        assert_eq!(&v[..22], b"manfid=0x2d0\0sromrev=11\0".get(..22).unwrap());
        assert_eq!(v.len() % 4, 0);
        let words = (v.len() / 4 - 1) as u32;
        let token = u32::from_le_bytes([v[v.len() - 4], v[v.len() - 3], v[v.len() - 2], v[v.len() - 1]]);
        assert_eq!(token, words | (!words & 0xffff) << 16);
        let v = nvram(b"a=1\n", Some([0xb8, 0x27, 0xeb, 0x01, 0x02, 0x03]));
        assert!(v.starts_with(b"a=1\0macaddr=b8:27:eb:01:02:03\0"));
        assert_eq!(reset_vector(&[0x78, 0x56, 0x34, 0x12, 9]), 0x1234_5678);
    }

    #[test]
    fn blob_pieces() {
        let blob: Vec<u8> = (0..3000u32).map(|i| i as u8).collect();
        let pieces = dload(&blob);
        assert_eq!(pieces.len(), 3);
        assert_eq!(&pieces[0][..2], &(DLOAD_FLAG_HANDLER_VER_1 | DLOAD_FLAG_BEGIN).to_le_bytes());
        assert_eq!(&pieces[2][..2], &(DLOAD_FLAG_HANDLER_VER_1 | DLOAD_FLAG_END).to_le_bytes());
        assert_eq!(pieces[1][2..4], DLOAD_TYPE_CLM.to_le_bytes());
        assert_eq!(pieces[2][4..8], 200u32.to_le_bytes());
        assert_eq!(&pieces[2][DLOAD_HDR_LEN..], &blob[2800..]);
    }
}
