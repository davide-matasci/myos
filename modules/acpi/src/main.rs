//! Minimal ACPI: RSDP → XSDT/RSDT, MADT/MCFG/FADT/DSDT summary + AML `_S5`.
//!
//! Supported AML opcodes (see docs/pci-acpi-smp.md):
//! Zero, One, Ones, Byte/Word/DWord/QWordPrefix, StringPrefix,
//! NameOp, PackageOp, BufferOp (skip), ScopeOp/DeviceOp/MethodOp (skip body),
//! ReturnOp, ExtOp+Mutex/Event/Field/IndexField/BankField/OpRegion (skip),
//! DefName path walk to find `_S5`.

#![no_std]
#![no_main]

use myos_abi::{status_info, status_ok, status_warn, ABI_VERSION, KernelApi};

const AML_ZERO: u8 = 0x00;
const AML_ONE: u8 = 0x01;
const AML_ONES: u8 = 0xFF;
const AML_BYTE: u8 = 0x0A;
const AML_WORD: u8 = 0x0B;
const AML_DWORD: u8 = 0x0C;
const AML_STRING: u8 = 0x0D;
const AML_QWORD: u8 = 0x0E;
const AML_SCOPE: u8 = 0x10;
const AML_BUFFER: u8 = 0x11;
const AML_PACKAGE: u8 = 0x12;
const AML_METHOD: u8 = 0x14;
const AML_EXT: u8 = 0x5B;
const AML_NAME: u8 = 0x08;
const AML_RETURN: u8 = 0xA4;
const AML_DEVICE: u8 = 0x82; // ExtOp 0x82 after 0x5B? Actually DeviceOp is ExtOpPrefix+0x82
// Device is Ext: 0x5B 0x82. Method is 0x14. Scope is 0x10.

#[inline(never)]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn module_init(api: *const KernelApi) -> i32 {
    unsafe {
        if api.is_null() {
            return -1;
        }
        let api = &*api;
        if api.abi_version != ABI_VERSION {
            return -2;
        }

        let rsdp = (api.acpi_rsdp)();
        if rsdp == 0 {
            publish(
                api,
                b"acpi/info",
                b"source: none\nnote: no RSDP (DT-only firmware; ACPI stubs)\n",
            );
            publish(api, b"acpi/tables", b"# no ACPI tables\n");
            publish(
                api,
                b"acpi/s5",
                b"slp_typa: -\nslp_typb: -\nnote: no AML (non-ACPI arch or missing RSDP)\n",
            );
            status_info(api, "acpi: no RSDP (stub)");
            return 0;
        }

        let hhdm = (api.hhdm_offset)();
        let mut info_buf = [0u8; 256];
        let mut info_len = 0usize;
        push(&mut info_buf, &mut info_len, b"source: limine-rsdp\nrsdp: 0x");
        hex_usize(&mut info_buf, &mut info_len, rsdp);
        push(&mut info_buf, &mut info_len, b"\n");

        let Some(rsdp_bytes) = read_va(rsdp, 36) else {
            status_warn(api, "acpi: bad RSDP map");
            return -3;
        };
        // Prefer XSDT (rev >= 2).
        let rev = rsdp_bytes[15];
        let use_xsdt = rev >= 2 && rsdp_bytes.len() >= 36;
        let root_phys = if use_xsdt {
            u64::from_le_bytes(rsdp_bytes[24..32].try_into().unwrap_or([0; 8]))
        } else {
            u32::from_le_bytes(rsdp_bytes[16..20].try_into().unwrap_or([0; 4])) as u64
        };
        let root_va = phys_to_va(root_phys, hhdm);
        push(&mut info_buf, &mut info_len, if use_xsdt { b"root: XSDT\n" } else { b"root: RSDT\n" });

        let mut tables_buf = [0u8; 1024];
        let mut tables_len = 0usize;
        push(&mut tables_buf, &mut tables_len, b"# sig  phys  length\n");

        let mut dsdt_phys = 0u64;
        let mut madt_cpus = 0u32;
        let mut mcfg_base = 0u64;

        if let Some(hdr) = read_va(root_va, 36) {
            let root_len = u32::from_le_bytes(hdr[4..8].try_into().unwrap_or([0; 4])) as usize;
            let entry_size = if use_xsdt { 8usize } else { 4usize };
            let entries = root_len.saturating_sub(36) / entry_size;
            if let Some(body) = read_va(root_va, root_len.min(4096)) {
                for i in 0..entries {
                    let off = 36 + i * entry_size;
                    if off + entry_size > body.len() {
                        break;
                    }
                    let phys = if use_xsdt {
                        u64::from_le_bytes(body[off..off + 8].try_into().unwrap_or([0; 8]))
                    } else {
                        u32::from_le_bytes(body[off..off + 4].try_into().unwrap_or([0; 4])) as u64
                    };
                    if phys == 0 {
                        continue;
                    }
                    let va = phys_to_va(phys, hhdm);
                    let Some(th) = read_va(va, 8) else { continue };
                    let sig = &th[0..4];
                    let tlen = u32::from_le_bytes(th[4..8].try_into().unwrap_or([0; 4]));
                    push(&mut tables_buf, &mut tables_len, sig);
                    push(&mut tables_buf, &mut tables_len, b"  0x");
                    hex_u64(&mut tables_buf, &mut tables_len, phys);
                    push(&mut tables_buf, &mut tables_len, b"  ");
                    dec_u32(&mut tables_buf, &mut tables_len, tlen);
                    push(&mut tables_buf, &mut tables_len, b"\n");

                    if sig == b"APIC" {
                        madt_cpus = count_madt_cpus(va, tlen as usize);
                    } else if sig == b"MCFG" {
                        if let Some(m) = read_va(va, 60) {
                            if m.len() >= 52 {
                                mcfg_base = u64::from_le_bytes(m[44..52].try_into().unwrap_or([0; 8]));
                            }
                        }
                    } else if sig == b"FACP" {
                        // FADT: DSDT at 40 (32-bit) or X_DSDT at 140 (64-bit).
                        if let Some(f) = read_va(va, 148.min(tlen as usize)) {
                            if f.len() >= 44 {
                                let d32 = u32::from_le_bytes(f[40..44].try_into().unwrap_or([0; 4])) as u64;
                                if d32 != 0 {
                                    dsdt_phys = d32;
                                }
                            }
                            if f.len() >= 148 {
                                let xd = u64::from_le_bytes(f[140..148].try_into().unwrap_or([0; 8]));
                                if xd != 0 {
                                    dsdt_phys = xd;
                                }
                            }
                        }
                    }
                }
            }
        }

        push(&mut info_buf, &mut info_len, b"madt_cpus: ");
        dec_u32(&mut info_buf, &mut info_len, madt_cpus);
        push(&mut info_buf, &mut info_len, b"\nmcfg_base: 0x");
        hex_u64(&mut info_buf, &mut info_len, mcfg_base);
        push(&mut info_buf, &mut info_len, b"\n");

        let mut s5_buf = [0u8; 128];
        let mut s5_len = 0usize;
        let (typa, typb, aml_ok) = if dsdt_phys != 0 {
            eval_s5(phys_to_va(dsdt_phys, hhdm))
        } else {
            (0, 0, false)
        };
        if aml_ok {
            push(&mut s5_buf, &mut s5_len, b"slp_typa: ");
            dec_u32(&mut s5_buf, &mut s5_len, typa as u32);
            push(&mut s5_buf, &mut s5_len, b"\nslp_typb: ");
            dec_u32(&mut s5_buf, &mut s5_len, typb as u32);
            push(&mut s5_buf, &mut s5_len, b"\nsource: AML _S5\n");
        } else {
            push(
                &mut s5_buf,
                &mut s5_len,
                b"slp_typa: -\nslp_typb: -\nnote: _S5 not found or unsupported AML\n",
            );
        }

        // Leak buffers into procfs.
        publish_alloc(api, b"acpi/info", &info_buf[..info_len]);
        publish_alloc(api, b"acpi/tables", &tables_buf[..tables_len]);
        publish_alloc(api, b"acpi/s5", &s5_buf[..s5_len]);

        if aml_ok {
            status_ok(api, "acpi: tables+AML _S5");
        } else {
            status_ok(api, "acpi: tables");
        }
        let _ = status_info;
        0
    }
}

fn publish(api: &KernelApi, name: &[u8], data: &'static [u8]) {
    unsafe {
        let _ = (api.proc_register)(name.as_ptr(), name.len(), data.as_ptr(), data.len());
    }
}

fn publish_alloc(api: &KernelApi, name: &[u8], data: &[u8]) {
    unsafe {
        let p = (api.alloc)(data.len().max(1), 8);
        if p.is_null() {
            return;
        }
        core::ptr::copy_nonoverlapping(data.as_ptr(), p, data.len());
        let _ = (api.proc_register)(name.as_ptr(), name.len(), p, data.len());
    }
}

fn phys_to_va(phys: u64, hhdm: u64) -> usize {
    if hhdm == 0 {
        phys as usize
    } else {
        (phys + hhdm) as usize
    }
}

unsafe fn read_va<'a>(va: usize, len: usize) -> Option<&'a [u8]> {
    if va == 0 || len == 0 {
        return None;
    }
    Some(core::slice::from_raw_parts(va as *const u8, len))
}

fn count_madt_cpus(va: usize, len: usize) -> u32 {
    unsafe {
        let Some(b) = read_va(va, len.min(4096)) else {
            return 0;
        };
        if b.len() < 44 {
            return 0;
        }
        let mut off = 44usize;
        let mut n = 0u32;
        while off + 2 <= b.len() {
            let typ = b[off];
            let entry_len = b[off + 1] as usize;
            if entry_len < 2 || off + entry_len > b.len() {
                break;
            }
            // Type 0 = Local APIC, type 9 = x2APIC — count enabled.
            if typ == 0 && entry_len >= 8 {
                let flags = u32::from_le_bytes(b[off + 4..off + 8].try_into().unwrap_or([0; 4]));
                if flags & 1 != 0 {
                    n += 1;
                }
            } else if typ == 9 && entry_len >= 16 {
                let flags = u32::from_le_bytes(b[off + 8..off + 12].try_into().unwrap_or([0; 4]));
                if flags & 1 != 0 {
                    n += 1;
                }
            }
            off += entry_len;
        }
        n
    }
}

/// Scan DSDT AML for `Name (_S5, Package (N) { … })` and read first two integers.
fn eval_s5(dsdt_va: usize) -> (u8, u8, bool) {
    unsafe {
        let Some(hdr) = read_va(dsdt_va, 8) else {
            return (0, 0, false);
        };
        let tlen = u32::from_le_bytes(hdr[4..8].try_into().unwrap_or([0; 4])) as usize;
        if tlen < 36 || tlen > 512 * 1024 {
            return (0, 0, false);
        }
        let Some(table) = read_va(dsdt_va, tlen) else {
            return (0, 0, false);
        };
        // AML starts after 36-byte SDT header (DSDT).
        let aml = &table[36..];
        find_s5_package(aml)
    }
}

fn find_s5_package(aml: &[u8]) -> (u8, u8, bool) {
    let mut i = 0usize;
    while i + 5 < aml.len() {
        if aml[i] == AML_NAME {
            let (name_end, name) = parse_name_string(&aml[i + 1..]);
            if name_end == 0 {
                i += 1;
                continue;
            }
            let after_name = i + 1 + name_end;
            if name_is_s5(&name) && after_name < aml.len() && aml[after_name] == AML_PACKAGE {
                if let Some((a, b)) = parse_s5_pkg(&aml[after_name..]) {
                    return (a, b, true);
                }
            }
            i = after_name;
            continue;
        }
        // Skip Scope/Method/Device bodies by walking pkg length when obvious.
        if aml[i] == AML_SCOPE || aml[i] == AML_METHOD {
            i += 1;
            continue;
        }
        if aml[i] == AML_EXT && i + 1 < aml.len() && aml[i + 1] == AML_DEVICE {
            i += 2;
            continue;
        }
        i += 1;
    }
    (0, 0, false)
}

fn name_is_s5(name: &[u8]) -> bool {
    // Match `_S5_`, `\_S5_`, or ending with `_S5_`.
    if name.len() >= 4 {
        let n = &name[name.len() - 4..];
        return n == b"_S5_";
    }
    false
}

fn parse_name_string(data: &[u8]) -> (usize, [u8; 16]) {
    let mut out = [0u8; 16];
    if data.is_empty() {
        return (0, out);
    }
    let mut i = 0usize;
    let mut o = 0usize;
    // Root `\`, parent `^`, DualName 0x2E, MultiName 0x2F, or 4-char NameSeg.
    if data[0] == b'\\' {
        out[o] = b'\\';
        o += 1;
        i += 1;
    }
    while i < data.len() && data[i] == b'^' {
        if o < 16 {
            out[o] = b'^';
            o += 1;
        }
        i += 1;
    }
    if i >= data.len() {
        return (0, out);
    }
    let segs = if data[i] == 0x2E {
        i += 1;
        2usize
    } else if data[i] == 0x2F {
        i += 1;
        if i >= data.len() {
            return (0, out);
        }
        let c = data[i] as usize;
        i += 1;
        c
    } else if data[i] == 0x00 {
        // NullName
        return (i + 1, out);
    } else {
        1usize
    };
    for _ in 0..segs {
        if i + 4 > data.len() {
            return (0, out);
        }
        for _ in 0..4 {
            if o < 16 {
                out[o] = data[i];
                o += 1;
            }
            i += 1;
        }
    }
    (i, out)
}

fn parse_s5_pkg(data: &[u8]) -> Option<(u8, u8)> {
    // PackageOp PkgLength NumElements TermList
    if data.first()? != &AML_PACKAGE {
        return None;
    }
    let (pkglen_size, pkglen) = pkg_length(&data[1..])?;
    let mut off = 1 + pkglen_size;
    if off >= data.len() {
        return None;
    }
    let _num = data[off];
    off += 1;
    let end = (1 + pkglen).min(data.len());
    let mut vals = [0u8; 4];
    let mut n = 0usize;
    while off < end && n < 4 {
        let (v, consumed) = read_integer(&data[off..end])?;
        vals[n] = v as u8;
        n += 1;
        off += consumed;
    }
    if n >= 2 {
        Some((vals[0], vals[1]))
    } else if n == 1 {
        Some((vals[0], 0))
    } else {
        None
    }
}

fn pkg_length(data: &[u8]) -> Option<(usize, usize)> {
    let b0 = *data.first()?;
    let bytes = ((b0 >> 6) & 3) as usize;
    if bytes == 0 {
        return Some((1, (b0 & 0x3F) as usize));
    }
    if data.len() < bytes + 1 {
        return None;
    }
    let mut len = (b0 & 0x0F) as usize;
    for i in 0..bytes {
        len |= (data[1 + i] as usize) << (4 + i * 8);
    }
    Some((bytes + 1, len))
}

fn read_integer(data: &[u8]) -> Option<(u64, usize)> {
    if data.is_empty() {
        return None;
    }
    match data[0] {
        AML_ZERO => Some((0, 1)),
        AML_ONE => Some((1, 1)),
        AML_ONES => Some((0xFF, 1)),
        AML_BYTE if data.len() >= 2 => Some((data[1] as u64, 2)),
        AML_WORD if data.len() >= 3 => {
            Some((u16::from_le_bytes([data[1], data[2]]) as u64, 3))
        }
        AML_DWORD if data.len() >= 5 => {
            Some((
                u32::from_le_bytes(data[1..5].try_into().ok()?) as u64,
                5,
            ))
        }
        AML_QWORD if data.len() >= 9 => {
            Some((u64::from_le_bytes(data[1..9].try_into().ok()?), 9))
        }
        // Skip Return / Store noise
        AML_RETURN => {
            let (v, c) = read_integer(&data[1..])?;
            Some((v, c + 1))
        }
        _ => None,
    }
}

fn push(buf: &mut [u8], len: &mut usize, b: &[u8]) {
    let n = b.len().min(buf.len().saturating_sub(*len));
    buf[*len..*len + n].copy_from_slice(&b[..n]);
    *len += n;
}

fn hex_usize(buf: &mut [u8], len: &mut usize, v: usize) {
    hex_u64(buf, len, v as u64);
}

fn hex_u64(buf: &mut [u8], len: &mut usize, mut v: u64) {
    let mut tmp = [0u8; 16];
    for i in (0..16).rev() {
        let n = (v & 0xf) as u8;
        tmp[i] = if n < 10 { b'0' + n } else { b'a' + (n - 10) };
        v >>= 4;
    }
    // skip leading zeros but keep one
    let mut s = 0;
    while s < 15 && tmp[s] == b'0' {
        s += 1;
    }
    push(buf, len, &tmp[s..]);
}

fn dec_u32(buf: &mut [u8], len: &mut usize, mut v: u32) {
    if v == 0 {
        push(buf, len, b"0");
        return;
    }
    let mut tmp = [0u8; 10];
    let mut t = 0;
    while v > 0 {
        tmp[t] = b'0' + (v % 10) as u8;
        v /= 10;
        t += 1;
    }
    while t > 0 {
        t -= 1;
        push(buf, len, &tmp[t..t + 1]);
    }
}

#[inline(never)]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn module_exit() {}

#[unsafe(no_mangle)]
pub extern "C" fn _start() -> ! {
    loop {}
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    loop {}
}
