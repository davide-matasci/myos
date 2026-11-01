//! A battery and an AC adapter for the x86 test boots: an SSDT, QEMU's
//! `-acpitable`, whose AML the acpi module's interpreter finds and
//! evaluates for `/proc/acpi/battery` (`docs/platform.md` "Batteries").
//! QEMU's PCs have neither; the table is written here, its AML encoded by
//! hand, as if compiled from:
//!
//! ```text
//! DefinitionBlock ("", "SSDT", 2, "MYOS", "BATTERY", 1) {
//!     Scope (\_SB) {
//!         Device (BAT0) {
//!             Name (_HID, EisaId ("PNP0C0A"))
//!             // Present when Match finds 2 at index 1; the Notify is
//!             // dropped (both at the step where a ThinkPad T15's AML
//!             // uses them: the devices' _STA and _INI).
//!             Method (_STA) {
//!                 Notify (BAT0, 0x80)
//!                 If (Match (Package () { 1, 2, 3 }, MEQ, 2, MTR, 0, 0) == 1) {
//!                     Return (0x1F)
//!                 }
//!                 Return (0)
//!             }
//!             Name (_BIF, Package () { 0, 50000, 40000, 1, 11100, 2000, 500,
//!                                      1, 1, "MYOS", "1", "LION", "myos" })
//!             Name (_BST, Package () { 1, 10000, 30000, 11100 })
//!         }
//!         Device (AC) {
//!             Name (_HID, "ACPI0003")
//!             Name (_PSR, Zero)
//!         }
//!     }
//! }
//! ```
//!
//! So the guest reads: BAT0 present, discharging at 10 W, 30 of its 40 Wh
//! left (75%, three hours), the adapter offline (`user/tests/kernel.sh`).

use std::path::PathBuf;
use std::process::Command;

const NAME_OP: u8 = 0x08;
const SCOPE_OP: u8 = 0x10;
const PACKAGE_OP: u8 = 0x12;
const BYTE_PREFIX: u8 = 0x0a;
const WORD_PREFIX: u8 = 0x0b;
const DWORD_PREFIX: u8 = 0x0c;
const STRING_PREFIX: u8 = 0x0d;
const DEVICE_OP: [u8; 2] = [0x5b, 0x82];
const METHOD_OP: u8 = 0x14;
const IF_OP: u8 = 0xa0;
const RETURN_OP: u8 = 0xa4;
const LEQUAL_OP: u8 = 0x93;
const MATCH_OP: u8 = 0x89;
const NOTIFY_OP: u8 = 0x86;
/// Match's comparisons: equal, always true.
const MEQ: u8 = 1;
const MTR: u8 = 0;

/// `body` after its PkgLength, which counts itself: one byte up to 63,
/// else the low nibble in the lead byte and the rest in the next ones.
fn pkg(body: Vec<u8>) -> Vec<u8> {
    let mut out = Vec::new();
    let mut extra = 0;
    let len = loop {
        let len = body.len() + 1 + extra;
        let fits = match extra {
            0 => len < 0x40,
            1 => len < 1 << 12,
            2 => len < 1 << 20,
            _ => true,
        };
        if fits {
            break len;
        }
        extra += 1;
    };
    if extra == 0 {
        out.push(len as u8);
    } else {
        out.push((extra << 6) as u8 | (len & 0xf) as u8);
        for i in 0..extra {
            out.push((len >> (4 + 8 * i)) as u8);
        }
    }
    out.extend(body);
    out
}

/// An integer, in the smallest encoding.
fn int(v: u32) -> Vec<u8> {
    match v {
        0 => vec![0x00],
        1 => vec![0x01],
        2..=0xff => vec![BYTE_PREFIX, v as u8],
        0x100..=0xffff => [&[WORD_PREFIX][..], &(v as u16).to_le_bytes()].concat(),
        _ => [&[DWORD_PREFIX][..], &v.to_le_bytes()].concat(),
    }
}

fn string(s: &str) -> Vec<u8> {
    [&[STRING_PREFIX], s.as_bytes(), &[0]].concat()
}

fn name(seg: &[u8; 4], value: Vec<u8>) -> Vec<u8> {
    [&[NAME_OP][..], seg, &value].concat()
}

fn package(items: Vec<Vec<u8>>) -> Vec<u8> {
    let mut body = vec![items.len() as u8];
    body.extend(items.concat());
    [vec![PACKAGE_OP], pkg(body)].concat()
}

/// A method of no arguments, not serialized.
fn method(seg: &[u8; 4], body: Vec<Vec<u8>>) -> Vec<u8> {
    [vec![METHOD_OP], pkg([seg.to_vec(), vec![0], body.concat()].concat())].concat()
}

fn device(seg: &[u8; 4], body: Vec<Vec<u8>>) -> Vec<u8> {
    [DEVICE_OP.to_vec(), pkg([seg.to_vec(), body.concat()].concat())].concat()
}

/// The table's bytes: the header, its checksum, the AML.
fn ssdt() -> Vec<u8> {
    // EisaId ("PNP0C0A").
    const PNP0C0A: u32 = 0x0a0c_d041;
    let bat = device(
        b"BAT0",
        vec![
            name(b"_HID", int(PNP0C0A)),
            method(
                b"_STA",
                vec![
                    [&[NOTIFY_OP][..], b"BAT0", &int(0x80)].concat(),
                    [
                        vec![IF_OP],
                        pkg([
                            vec![LEQUAL_OP, MATCH_OP],
                            package(vec![int(1), int(2), int(3)]),
                            vec![MEQ],
                            int(2),
                            vec![MTR],
                            int(0),
                            int(0),
                            int(1),
                            vec![RETURN_OP],
                            int(0x1f),
                        ]
                        .concat()),
                    ]
                    .concat(),
                    [vec![RETURN_OP], int(0)].concat(),
                ],
            ),
            name(
                b"_BIF",
                package(vec![
                    int(0),
                    int(50000),
                    int(40000),
                    int(1),
                    int(11100),
                    int(2000),
                    int(500),
                    int(1),
                    int(1),
                    string("MYOS"),
                    string("1"),
                    string("LION"),
                    string("myos"),
                ]),
            ),
            name(b"_BST", package(vec![int(1), int(10000), int(30000), int(11100)])),
        ],
    );
    let ac = device(b"AC__", vec![name(b"_HID", string("ACPI0003")), name(b"_PSR", int(0))]);
    let aml = [vec![SCOPE_OP], pkg([b"\\_SB_".to_vec(), bat, ac].concat())].concat();

    let mut t = Vec::new();
    t.extend(b"SSDT");
    t.extend(((36 + aml.len()) as u32).to_le_bytes());
    t.push(2); // revision
    t.push(0); // checksum, below
    t.extend(b"MYOS  ");
    t.extend(b"BATTERY ");
    t.extend(1u32.to_le_bytes());
    t.extend(b"MYOS");
    t.extend(1u32.to_le_bytes());
    t.extend(aml);
    t[9] = 0u8.wrapping_sub(t.iter().fold(0u8, |a, b| a.wrapping_add(*b)));
    t
}

/// Write the table under `target/` and hand it to QEMU.
pub fn add(cmd: &mut Command) {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/acpi-battery.ssdt");
    if let Err(e) = std::fs::write(&path, ssdt()) {
        eprintln!("warning: no test battery ({}: {e})", path.display());
        return;
    }
    cmd.arg("-acpitable").arg(format!("file={}", path.display()));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checksum_and_lengths() {
        let t = ssdt();
        assert_eq!(t.iter().fold(0u8, |a, b| a.wrapping_add(*b)), 0);
        assert_eq!(u32::from_le_bytes(t[4..8].try_into().unwrap()) as usize, t.len());
        assert_eq!(pkg(vec![0; 10]).len(), 11);
        // 0x50 bytes need two: 0x52 counts both.
        assert_eq!(&pkg(vec![0; 0x50])[..2], &[0x42, 0x05]);
    }
}
