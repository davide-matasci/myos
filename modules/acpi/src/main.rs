//! Minimal ACPI: RSDP → XSDT/RSDT, MADT/MCFG/FADT/DSDT summary + AML `_S5`,
//! and the ACPI power-off and reset methods (`power`).
//!
//! On x86_64 the `acpi` crate's AML interpreter runs the firmware's AML
//! (`aml`): the PCI interrupt routes (`_PRT`) for the kernel, and `_S5`.
//! Without it (another arch, or the interpreter failed) `_S5` comes from
//! the AML read (see docs/platform.md): `Name (_S5, Package ...)`
//! found by its bytes, its integers (Zero, One, Ones, Byte/Word/DWord/
//! QWordPrefix, ReturnOp) read.

#![no_std]
#![no_main]
#![deny(unsafe_op_in_unsafe_fn)]

extern crate alloc;

use core::alloc::{GlobalAlloc, Layout};

use myos_abi::{status_info, status_ok, status_warn, ApiCell, ABI_VERSION, KernelApi};

#[cfg(target_arch = "x86_64")]
mod aml;
#[cfg(target_arch = "x86_64")]
mod battery;
mod power;

static API: ApiCell = ApiCell::new();

#[cfg_attr(not(target_arch = "x86_64"), allow(dead_code))]
fn api() -> &'static KernelApi {
    API.get()
}

/// The kernel heap, through the ABI: the interpreter's namespace.
struct KernelHeap;

unsafe impl GlobalAlloc for KernelHeap {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        API.get().alloc(layout.size(), layout.align())
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { (API.get().dealloc)(ptr, layout.size(), layout.align()) }
    }
}

#[global_allocator]
static HEAP: KernelHeap = KernelHeap;

const AML_ZERO: u8 = 0x00;
const AML_ONE: u8 = 0x01;
const AML_ONES: u8 = 0xFF;
const AML_BYTE: u8 = 0x0A;
const AML_WORD: u8 = 0x0B;
const AML_DWORD: u8 = 0x0C;
const AML_STRING: u8 = 0x0D;
const AML_QWORD: u8 = 0x0E;
const AML_BUFFER: u8 = 0x11;
const AML_PACKAGE: u8 = 0x12;
const AML_NAME: u8 = 0x08;
const AML_RETURN: u8 = 0xA4;

#[inline(never)]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn module_init(api_ptr: *const KernelApi) -> i32 {
    let Some(api) = (unsafe { api_ptr.as_ref() }) else {
        return -1;
    };
    if api.abi_version != ABI_VERSION {
        return -2;
    }
    unsafe { API.set(api_ptr) };

    let rsdp = api.acpi_rsdp();
    if rsdp == 0 {
        publish(
            api,
            "acpi/info",
            b"source: none\nnote: no RSDP (DT-only firmware; ACPI stubs)\n",
        );
        publish(api, "acpi/tables", b"# no ACPI tables\n");
        publish(
            api,
            "acpi/s5",
            b"slp_typa: -\nslp_typb: -\nnote: no AML (non-ACPI arch or missing RSDP)\n",
        );
        status_info(api, "acpi: no RSDP (stub)");
        return 0;
    }

    let hhdm = api.hhdm_offset();
    let mut info_buf = [0u8; 256];
    let mut info_len = 0usize;
    push(&mut info_buf, &mut info_len, b"source: limine-rsdp\nrsdp: 0x");
    hex_usize(&mut info_buf, &mut info_len, rsdp);
    push(&mut info_buf, &mut info_len, b"\n");

    let Some(rsdp_bytes) = (unsafe { read_va(rsdp, 36) }) else {
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
    let mut fadt: &[u8] = &[];
    // Every table the root lists, for `/proc/acpi/dump`.
    let mut listed: alloc::vec::Vec<(usize, usize)> = alloc::vec::Vec::new();
    let mut madt_cpus = 0u32;
    let mut mcfg_base = 0u64;

    if let Some(hdr) = unsafe { read_va(root_va, 36) } {
        let root_len = u32::from_le_bytes(hdr[4..8].try_into().unwrap_or([0; 4])) as usize;
        let entry_size = if use_xsdt { 8usize } else { 4usize };
        let entries = root_len.saturating_sub(36) / entry_size;
        if let Some(body) = unsafe { read_va(root_va, root_len.min(4096)) } {
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
                let Some(th) = (unsafe { read_va(va, 8) }) else { continue };
                let sig = &th[0..4];
                let tlen = u32::from_le_bytes(th[4..8].try_into().unwrap_or([0; 4]));
                listed.push((va, tlen as usize));
                push(&mut tables_buf, &mut tables_len, sig);
                push(&mut tables_buf, &mut tables_len, b"  0x");
                hex_u64(&mut tables_buf, &mut tables_len, phys);
                push(&mut tables_buf, &mut tables_len, b"  ");
                dec_u32(&mut tables_buf, &mut tables_len, tlen);
                push(&mut tables_buf, &mut tables_len, b"\n");

                if sig == b"APIC" {
                    madt_cpus = count_madt_cpus(va, tlen as usize);
                } else if sig == b"MCFG" {
                    if let Some(m) = unsafe { read_va(va, 60) } {
                        if m.len() >= 52 {
                            mcfg_base = u64::from_le_bytes(m[44..52].try_into().unwrap_or([0; 8]));
                        }
                    }
                } else if sig == b"FACP" {
                    // FADT: DSDT at 40 (32-bit) or X_DSDT at 140 (64-bit).
                    if let Some(f) = unsafe { read_va(va, (tlen as usize).min(4096)) } {
                        fadt = f;
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
    #[cfg_attr(not(target_arch = "x86_64"), allow(unused_mut))]
    let (mut typa, mut typb, mut aml_ok) = if dsdt_phys != 0 {
        eval_s5(phys_to_va(dsdt_phys, hhdm))
    } else {
        (0, 0, false)
    };
    #[cfg_attr(not(target_arch = "x86_64"), allow(unused_mut))]
    let mut source: &[u8] = b"AML _S5 scan";
    #[cfg(target_arch = "x86_64")]
    {
        let report = interpret(api, rsdp as u64, hhdm);
        if let Some((a, b)) = report.0 {
            (typa, typb, aml_ok) = (a, b, true);
            source = b"AML interpreter";
        }
        publish_alloc(api, "acpi/aml", report.1.as_bytes());
    }
    if aml_ok {
        push(&mut s5_buf, &mut s5_len, b"slp_typa: ");
        dec_u32(&mut s5_buf, &mut s5_len, typa as u32);
        push(&mut s5_buf, &mut s5_len, b"\nslp_typb: ");
        dec_u32(&mut s5_buf, &mut s5_len, typb as u32);
        push(&mut s5_buf, &mut s5_len, b"\nsource: ");
        push(&mut s5_buf, &mut s5_len, source);
        push(&mut s5_buf, &mut s5_len, b"\n");
    } else {
        push(
            &mut s5_buf,
            &mut s5_len,
            b"slp_typa: -\nslp_typb: -\nnote: _S5 not found or unsupported AML\n",
        );
    }

    power::register(api, fadt, aml_ok.then_some((typa, typb)));

    // Leak buffers into procfs.
    let dsdt = (dsdt_phys != 0).then(|| phys_to_va(dsdt_phys, hhdm));
    publish_alloc(api, "acpi/dump", &dump(dsdt, &listed));
    publish_alloc(api, "acpi/info", &info_buf[..info_len]);
    publish_alloc(api, "acpi/tables", &tables_buf[..tables_len]);
    publish_alloc(api, "acpi/s5", &s5_buf[..s5_len]);

    if aml_ok {
        status_ok(api, "acpi: tables+AML _S5");
    } else {
        status_ok(api, "acpi: tables");
    }
    let _ = status_info;
    0
}

/// Run the interpreter on the tables of the RSDP at `rsdp` (an address of
/// the higher half): `_S5`'s sleep types when it found them, and the
/// report for `/proc/acpi/aml`. The deadline bounds the wait; the PCI
/// routes went to the kernel as found.
#[cfg(target_arch = "x86_64")]
fn interpret(api: &KernelApi, rsdp: u64, hhdm: u64) -> (Option<(u8, u8)>, alloc::string::String) {
    let phys = if hhdm != 0 && rsdp >= hhdm { rsdp - hhdm } else { rsdp };
    if !aml::start(api, phys) {
        return (None, "interpreter: no thread\n".into());
    }
    let (s5, report) = aml::wait(api, api.monotonic_ns() + aml::DEADLINE_NS);
    if s5.is_none() {
        status_warn(api, "acpi: AML interpreter failed (/proc/acpi/aml)");
    }
    (s5, report)
}

fn publish(api: &KernelApi, name: &str, data: &'static [u8]) {
    let _ = api.proc_register(name, data);
}

fn publish_alloc(api: &KernelApi, name: &str, data: &[u8]) {
    let p = api.alloc(data.len().max(1), 8);
    if p.is_null() {
        return;
    }
    // SAFETY: `p` is a fresh allocation of at least `data.len()` bytes.
    let copy = unsafe { core::slice::from_raw_parts_mut(p, data.len()) };
    copy.copy_from_slice(data);
    let _ = api.proc_register(name, copy);
}

fn phys_to_va(phys: u64, hhdm: u64) -> usize {
    if hhdm == 0 {
        phys as usize
    } else {
        (phys + hhdm) as usize
    }
}

/// # Safety
///
/// A non-zero `va` maps `len` readable bytes (a firmware table in the HHDM).
unsafe fn read_va<'a>(va: usize, len: usize) -> Option<&'a [u8]> {
    if va == 0 || len == 0 {
        return None;
    }
    Some(unsafe { core::slice::from_raw_parts(va as *const u8, len) })
}

fn count_madt_cpus(va: usize, len: usize) -> u32 {
    let Some(b) = (unsafe { read_va(va, len.min(4096)) }) else {
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

/// The largest table `/proc/acpi/dump` takes (a laptop's DSDT is a few
/// hundred KiB).
const DUMP_TABLE_MAX: usize = 4 << 20;

/// `/proc/acpi/dump`: the DSDT (at `dsdt`) and every table the root lists
/// (`listed`: address, length), each whole, header and all, one after the
/// other: the firmware's AML for a disassembler on another machine
/// (`docs/platform.md` "The tables"). Each table's length is in its
/// header, which is how they are told apart.
fn dump(dsdt: Option<usize>, listed: &[(usize, usize)]) -> alloc::vec::Vec<u8> {
    let mut out = alloc::vec::Vec::new();
    let table_len = |va: usize| -> Option<usize> {
        let hdr = unsafe { read_va(va, 8) }?;
        let len = u32::from_le_bytes(hdr[4..8].try_into().ok()?) as usize;
        (36..=DUMP_TABLE_MAX).contains(&len).then_some(len)
    };
    let dsdt = dsdt.and_then(|va| Some((va, table_len(va)?)));
    for (va, len) in dsdt.into_iter().chain(listed.iter().copied()) {
        if !(36..=DUMP_TABLE_MAX).contains(&len) {
            continue;
        }
        if let Some(table) = unsafe { read_va(va, len) } {
            out.extend_from_slice(table);
        }
    }
    out
}

/// Scan DSDT AML for `Name (_S5, Package (N) { … })` and read first two integers.
fn eval_s5(dsdt_va: usize) -> (u8, u8, bool) {
    let Some(hdr) = (unsafe { read_va(dsdt_va, 8) }) else {
        return (0, 0, false);
    };
    let tlen = u32::from_le_bytes(hdr[4..8].try_into().unwrap_or([0; 4])) as usize;
    if tlen < 36 || tlen > 512 * 1024 {
        return (0, 0, false);
    }
    let Some(table) = (unsafe { read_va(dsdt_va, tlen) }) else {
        return (0, 0, false);
    };
    // AML starts after 36-byte SDT header (DSDT).
    let aml = &table[36..];
    find_s5_package(aml)
}

/// `Name (_S5, Package (N) {SLP_TYPa, SLP_TYPb, ...})`: the name segment
/// `_S5_` right after a NameOp (the root prefix `\` may come between),
/// followed by a PackageOp. Found by looking for those bytes rather than by
/// walking the opcodes, which would need every one of them to stay in step
/// (a 0x08 in a buffer is no NameOp).
fn find_s5_package(aml: &[u8]) -> (u8, u8, bool) {
    for i in 1..aml.len().saturating_sub(4) {
        if &aml[i..i + 4] != b"_S5_" {
            continue;
        }
        let named = aml[i - 1] == AML_NAME || (aml[i - 1] == b'\\' && i >= 2 && aml[i - 2] == AML_NAME);
        if named && aml[i + 4] == AML_PACKAGE {
            if let Some((a, b)) = parse_s5_pkg(&aml[i + 4..]) {
                return (a, b, true);
            }
        }
    }
    (0, 0, false)
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
    // The interpreter's thread sleeps for good: the module's init goes on
    // without it.
    #[cfg(target_arch = "x86_64")]
    if aml::panicked(_info) {
        let api = api();
        loop {
            api.task_sleep_until(api.monotonic_ns() + 1_000_000_000_000);
        }
    }
    loop {}
}
