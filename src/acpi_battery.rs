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
//!             // Present when Match finds 2 at index 1 (through LNot and
//!             // Not, which the crate got wrong); the Notify is
//!             // dropped (both at the step where a ThinkPad T15's AML
//!             // uses them: the devices' _STA and _INI).
//!             Method (_STA) {
//!                 Notify (BAT0, 0x80)
//!                 If (LNot (Match (Package () { 1, 2, 3 }, MEQ, 2, MTR, 0, 0) != 1)) {
//!                     Return (And (Not (0xFFFFFFE0), 0x1F))
//!                 }
//!                 Return (0)
//!             }
//!             // The unit (mWh: 0) through a loop's Increment (Local0),
//!             // a T15's loop turning a number into digits (its battery's
//!             // serial number: Divide's remainder first, the quotient
//!             // second) and a store through Index, the package's 1 (mAh)
//!             // else.
//!             Name (BIFP, Package () { 1, 50000, 40000, 1, 11100, 2000, 500,
//!                                      1, 1, "MYOS", "1", "LION", "myos" })
//!             Name (SERN, Buffer () { 0, 0, 0 })
//!             Method (_BIF) {
//!                 Store (Zero, Local0)
//!                 While (LLess (Local0, 3)) { Increment (Local0) }
//!                 Store (Subtract (Local0, 3), Local3)
//!                 Store (123, Local0)
//!                 Store (2, Local2)
//!                 While (Local0) {
//!                     Divide (Local0, 10, Local1, Local0)
//!                     Add (Local1, 0x30, Index (SERN, Local2))
//!                     Decrement (Local2)
//!                 }
//!                 // Local2 is -1 once the three digits are in.
//!                 Store (Add (Local3, Add (Local2, 1)), Index (BIFP, 0))
//!                 Return (BIFP)
//!             }
//!             Name (_BST, Package () { 1, 10000, 30000, 11100 })
//!         }
//!         Device (AC) {
//!             Name (_HID, "ACPI0003")
//!             // A _STA that returns nothing (falls off its end), as a
//!             // ThinkPad T15's has one: the device counts as present.
//!             Method (_STA) { If (Zero) { Return (0x0F) } }
//!             // DerefOf, an Index used as an integer, a store through an
//!             // Index, stores into a named buffer and string and a
//!             // comparison of 16-byte buffers (a UUID's), all of which
//!             // the crate got wrong.
//!             // _INI (run at boot, as a T15's are) stores; _PSR reads
//!             // back: offline (0) only if the stores landed.
//!             Name (PKG, Package () { 5 })
//!             Name (BUF, Buffer () { 5, 6 })
//!             Name (STR, "AC")
//!             Name (UUID, Buffer () { 0, 1, ..., 15 })
//!             Method (_INI) {
//!                 If (LEqual (UUID, Buffer () { 0, 1, ..., 15 })) {
//!                     Store (Zero, Index (PKG, 0))
//!                 }
//!                 Store (0x0700, BUF)
//!                 Store (BUF, STR)
//!             }
//!             Method (_PSR) {
//!                 Return (Add (Add (DerefOf (Index (PKG, 0)),
//!                                   Index (Package () { 0 }, 0)),
//!                              DerefOf (Index (BUF, 0))))
//!             }
//!         }
//!         // An adapter whose _PSR fails (a name nothing defines): its
//!         // line says why.
//!         Device (AC2) {
//!             Name (_HID, "ACPI0003")
//!             Method (_PSR) { Return (NONE) }
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
const BUFFER_OP: u8 = 0x11;
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
const LNOT_OP: u8 = 0x92;
const NOT_OP: u8 = 0x80;
const AND_OP: u8 = 0x7b;
/// A target that discards: an op's result only goes to its caller.
const NULL_NAME: u8 = 0x00;
const ADD_OP: u8 = 0x72;
const DEREF_OF_OP: u8 = 0x83;
const INDEX_OP: u8 = 0x88;
const STORE_OP: u8 = 0x70;
const WHILE_OP: u8 = 0xa2;
const LLESS_OP: u8 = 0x95;
const INCREMENT_OP: u8 = 0x75;
const SUBTRACT_OP: u8 = 0x74;
const DIVIDE_OP: u8 = 0x78;
const DECREMENT_OP: u8 = 0x76;
const LOCAL0: u8 = 0x60;
const LOCAL1: u8 = 0x61;
const LOCAL2: u8 = 0x62;
const LOCAL3: u8 = 0x63;
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

/// A buffer of these bytes, its size their count.
fn buffer(bytes: &[u8]) -> Vec<u8> {
    [vec![BUFFER_OP], pkg([int(bytes.len() as u32), bytes.to_vec()].concat())].concat()
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
                            // LNot (LNotEqual (...)), LNotEqual being LNot (LEqual).
                            vec![LNOT_OP, LNOT_OP, LEQUAL_OP, MATCH_OP],
                            package(vec![int(1), int(2), int(3)]),
                            vec![MEQ],
                            int(2),
                            vec![MTR],
                            int(0),
                            int(0),
                            int(1),
                            vec![RETURN_OP, AND_OP, NOT_OP],
                            int(0xffff_ffe0),
                            vec![NULL_NAME],
                            int(0x1f),
                            vec![NULL_NAME],
                        ]
                        .concat()),
                    ]
                    .concat(),
                    [vec![RETURN_OP], int(0)].concat(),
                ],
            ),
            name(
                b"BIFP",
                package(vec![
                    int(1),
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
            name(b"SERN", buffer(&[0, 0, 0])),
            method(
                b"_BIF",
                vec![
                    [vec![STORE_OP], int(0), vec![LOCAL0]].concat(),
                    [vec![WHILE_OP], pkg([vec![LLESS_OP, LOCAL0], int(3), vec![INCREMENT_OP, LOCAL0]].concat())]
                        .concat(),
                    [vec![STORE_OP, SUBTRACT_OP, LOCAL0], int(3), vec![NULL_NAME, LOCAL3]].concat(),
                    [vec![STORE_OP], int(123), vec![LOCAL0]].concat(),
                    [vec![STORE_OP], int(2), vec![LOCAL2]].concat(),
                    [
                        vec![WHILE_OP],
                        pkg([
                            vec![LOCAL0, DIVIDE_OP, LOCAL0],
                            int(10),
                            vec![LOCAL1, LOCAL0, ADD_OP, LOCAL1],
                            int(0x30),
                            vec![INDEX_OP],
                            b"SERN".to_vec(),
                            vec![LOCAL2, NULL_NAME, DECREMENT_OP, LOCAL2],
                        ]
                        .concat()),
                    ]
                    .concat(),
                    [vec![STORE_OP, ADD_OP, LOCAL3, ADD_OP, LOCAL2], int(1), vec![NULL_NAME, NULL_NAME, INDEX_OP]]
                        .concat(),
                    [b"BIFP".to_vec(), int(0), vec![NULL_NAME]].concat(),
                    [&[RETURN_OP][..], b"BIFP"].concat(),
                ],
            ),
            name(b"_BST", package(vec![int(1), int(10000), int(30000), int(11100)])),
        ],
    );
    let uuid: Vec<u8> = (0..16).collect();
    let ac = device(
        b"AC__",
        vec![
            name(b"_HID", string("ACPI0003")),
            method(b"_STA", vec![[vec![IF_OP], pkg([int(0), vec![RETURN_OP], int(0x0f)].concat())].concat()]),
            name(b"PKG_", package(vec![int(5)])),
            name(b"BUF_", buffer(&[5, 6])),
            name(b"STR_", string("AC")),
            name(b"UUID", buffer(&uuid)),
            method(
                b"_INI",
                vec![
                    [
                        vec![IF_OP],
                        pkg([
                            vec![LEQUAL_OP],
                            b"UUID".to_vec(),
                            buffer(&uuid),
                            vec![STORE_OP],
                            int(0),
                            vec![INDEX_OP],
                            b"PKG_".to_vec(),
                            int(0),
                            vec![NULL_NAME],
                        ]
                        .concat()),
                    ]
                    .concat(),
                    [&[STORE_OP][..], &int(0x0700), b"BUF_"].concat(),
                    [&[STORE_OP][..], b"BUF_", b"STR_"].concat(),
                ],
            ),
            method(
                b"_PSR",
                vec![[
                    vec![RETURN_OP, ADD_OP, ADD_OP, DEREF_OF_OP, INDEX_OP],
                    b"PKG_".to_vec(),
                    int(0),
                    vec![NULL_NAME, INDEX_OP],
                    package(vec![int(0)]),
                    int(0),
                    vec![NULL_NAME, NULL_NAME, DEREF_OF_OP, INDEX_OP],
                    b"BUF_".to_vec(),
                    int(0),
                    vec![NULL_NAME, NULL_NAME],
                ]
                .concat()],
            ),
        ],
    );
    let ac2 = device(
        b"AC2_",
        vec![name(b"_HID", string("ACPI0003")), method(b"_PSR", vec![[&[RETURN_OP][..], b"NONE"].concat()])],
    );
    let aml = [vec![SCOPE_OP], pkg([b"\\_SB_".to_vec(), bat, ac, ac2].concat())].concat();

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
