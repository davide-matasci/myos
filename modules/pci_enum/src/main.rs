//! Full PCI configuration-space enumeration → `/proc/pci`.
//!
//! Lines keep hex BDF / vendor:device / class:sub:prog, and append short
//! human names when known (class/subclass + a small QEMU/virt ID table).
//! Unknown IDs stay hex-only.
//!
//! Write `rescan` (optionally with a trailing newline) to `/proc/pci` to
//! re-enumerate config space and refresh the node. Devices that disappeared
//! are dropped from the listing. The kernel also re-runs its NVMe PCI probe
//! on rescan (idempotent for already-attached controllers). virtio-net has
//! no probe-again path yet and stays boot-bound. No ACPI/QEMU hotplug IRQ
//! wiring — on-demand write is the trigger.
//!
//! Name strings are pushed from per-arm stack copies of byte literals so
//! RISC-V/AArch64 `relocation-model=static` ET_EXEC modules never load
//! unrebased `&'static str` addresses from a match pointer table (those
//! point into .rodata; the loader only rebases PF_X pointers).

#![no_std]
#![no_main]

use core::sync::atomic::{AtomicPtr, Ordering};
use myos_abi::{status_info, status_ok, status_warn, ABI_VERSION, KernelApi};

const MAX_DEV: usize = 64;
const BUF_CAP: usize = 8192;
const LINE_CAP: usize = 160;
const WRITE_CAP: usize = 64;

static API: AtomicPtr<KernelApi> = AtomicPtr::new(core::ptr::null_mut());
static BUF: AtomicPtr<u8> = AtomicPtr::new(core::ptr::null_mut());

#[inline(never)]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn module_init(api: *const KernelApi) -> i32 {
    unsafe {
        if api.is_null() {
            return -1;
        }
        let api_ref = &*api;
        if api_ref.abi_version != ABI_VERSION {
            return -2;
        }

        let buf = (api_ref.alloc)(BUF_CAP, 8);
        if buf.is_null() {
            status_warn(api_ref, "pci_enum: alloc");
            return -3;
        }
        API.store(api.cast_mut(), Ordering::Release);
        BUF.store(buf, Ordering::Release);

        let found = publish(api_ref, buf);
        let name = b"pci";
        let wr = (api_ref.proc_set_writer)(name.as_ptr(), name.len(), Some(pci_proc_write));
        if wr != 0 {
            status_warn(api_ref, "pci_enum: proc_set_writer");
            // Still usable read-only if writer attach failed.
        }
        let mut msg = [0u8; 32];
        let ml = format_count(&mut msg, found);
        status_ok(api_ref, core::str::from_utf8(&msg[..ml]).unwrap_or("pci"));
        let _ = status_info;
        0
    }
}

/// `write(2)` handler for `/proc/pci`. Accepts `rescan` (+ optional whitespace/newline).
unsafe extern "C" fn pci_proc_write(data: *const u8, data_len: usize) -> i32 {
    // Copy the command onto the stack so we never form a slice from an
    // untrusted C pointer (CodeQL rust/access-invalid-pointer) and so a
    // concurrent overwrite of the syscall buffer cannot race the parse.
    if data_len > WRITE_CAP {
        return -1;
    }
    let mut tmp = [0u8; WRITE_CAP];
    if data_len > 0 {
        if data.is_null() {
            return -1;
        }
        unsafe {
            core::ptr::copy_nonoverlapping(data, tmp.as_mut_ptr(), data_len);
        }
    }
    if !is_rescan_cmd(&tmp[..data_len]) {
        return -1;
    }
    let api = API.load(Ordering::Acquire);
    let buf = BUF.load(Ordering::Acquire);
    let Some(api_ref) = (unsafe { api.as_ref() }) else {
        return -1;
    };
    if buf.is_null() {
        return -1;
    }
    let _found = publish(api_ref, buf);
    data_len as i32
}

fn is_rescan_cmd(raw: &[u8]) -> bool {
    // Trim leading/trailing ASCII whitespace.
    let mut s = raw;
    while let Some((&b, rest)) = s.split_first() {
        if b == b' ' || b == b'\t' || b == b'\n' || b == b'\r' {
            s = rest;
        } else {
            break;
        }
    }
    while let Some((&b, rest)) = s.split_last() {
        if b == b' ' || b == b'\t' || b == b'\n' || b == b'\r' {
            s = rest;
        } else {
            break;
        }
    }
    s == b"rescan"
}

/// Walk config space, format `/proc/pci`, and (re)register the node.
/// Full rebuild drops devices that are gone since the last snapshot.
fn publish(api: &KernelApi, buf: *mut u8) -> usize {
    unsafe {
        let out = core::slice::from_raw_parts_mut(buf, BUF_CAP);
        let mut len = 0usize;
        push_str(
            out,
            &mut len,
            "# bus:slot.func vendor:device [name] class:sub:prog [class]\n",
        );
        push_str(
            out,
            &mut len,
            "# write \"rescan\" to re-enumerate (boot snapshot + on-demand)\n",
        );

        let mut found = 0usize;
        for bus in 0u8..32 {
            for slot in 0u8..32 {
                let id0 = (api.pci_cfg_read32)(bus, slot, 0, 0);
                if id0 as u16 == 0xFFFF {
                    continue;
                }
                let hdr = ((api.pci_cfg_read32)(bus, slot, 0, 0x0C) >> 16) as u8;
                let funcs = if hdr & 0x80 != 0 { 8u8 } else { 1u8 };
                for func in 0..funcs {
                    let id = (api.pci_cfg_read32)(bus, slot, func, 0);
                    let vend = id as u16;
                    if vend == 0xFFFF {
                        continue;
                    }
                    let dev = (id >> 16) as u16;
                    let classw = (api.pci_cfg_read32)(bus, slot, func, 0x08);
                    let class = (classw >> 24) as u8;
                    let sub = (classw >> 16) as u8;
                    let prog = (classw >> 8) as u8;
                    let mut line = [0u8; LINE_CAP];
                    let n = format_dev(&mut line, bus, slot, func, vend, dev, class, sub, prog);
                    push_bytes(out, &mut len, &line[..n]);
                    push_str(out, &mut len, "\n");
                    found += 1;
                    if found >= MAX_DEV || len + LINE_CAP >= BUF_CAP {
                        break;
                    }
                }
                if found >= MAX_DEV || len + LINE_CAP >= BUF_CAP {
                    break;
                }
            }
            if found >= MAX_DEV || len + LINE_CAP >= BUF_CAP {
                break;
            }
        }

        let name = b"pci";
        let rc = (api.proc_register)(name.as_ptr(), name.len(), buf, len);
        if rc != 0 {
            status_warn(api, "pci_enum: proc_register");
        }
        found
    }
}

fn push_str(out: &mut [u8], len: &mut usize, s: &str) {
    push_bytes(out, len, s.as_bytes());
}

fn push_bytes(out: &mut [u8], len: &mut usize, b: &[u8]) {
    let n = b.len().min(out.len().saturating_sub(*len));
    out[*len..*len + n].copy_from_slice(&b[..n]);
    *len += n;
}

fn hex_u8(dst: &mut [u8], off: usize, v: u8) -> usize {
    const H: &[u8; 16] = b"0123456789abcdef";
    dst[off] = H[(v >> 4) as usize];
    dst[off + 1] = H[(v & 0xf) as usize];
    off + 2
}

fn hex_u16(dst: &mut [u8], off: usize, v: u16) -> usize {
    let mut o = off;
    o = hex_u8(dst, o, (v >> 8) as u8);
    hex_u8(dst, o, v as u8)
}

fn push_ascii_bytes(dst: &mut [u8], off: usize, b: &[u8]) -> usize {
    let n = b.len().min(dst.len().saturating_sub(off));
    dst[off..off + n].copy_from_slice(&b[..n]);
    off + n
}

/// Append ` name` using a stack-local copy of the literal so the address we
/// read is never an unrebased absolute pointer from a match string table.
fn push_spaced_name(line: &mut [u8], mut o: usize, name: &[u8]) -> usize {
    if o >= line.len() {
        return o;
    }
    line[o] = b' ';
    o += 1;
    push_ascii_bytes(line, o, name)
}

fn push_device_name(line: &mut [u8], o: usize, vend: u16, dev: u16) -> usize {
    // Each arm copies the literal onto the stack (PC-relative load of bytes)
    // before appending — do not `return Some("...")` (absolute .rodata ptrs).
    match (vend, dev) {
        (0x1af4, 0x1000) | (0x1af4, 0x1041) => {
            let n = *b"virtio-net";
            push_spaced_name(line, o, &n)
        }
        (0x1af4, 0x1001) | (0x1af4, 0x1042) => {
            let n = *b"virtio-blk";
            push_spaced_name(line, o, &n)
        }
        (0x1af4, 0x1002) | (0x1af4, 0x1045) => {
            let n = *b"virtio-balloon";
            push_spaced_name(line, o, &n)
        }
        (0x1af4, 0x1003) | (0x1af4, 0x1043) => {
            let n = *b"virtio-console";
            push_spaced_name(line, o, &n)
        }
        (0x1af4, 0x1004) | (0x1af4, 0x1048) => {
            let n = *b"virtio-scsi";
            push_spaced_name(line, o, &n)
        }
        (0x1af4, 0x1005) | (0x1af4, 0x1044) => {
            let n = *b"virtio-rng";
            push_spaced_name(line, o, &n)
        }
        (0x1af4, 0x1009) | (0x1af4, 0x1049) => {
            let n = *b"virtio-9p";
            push_spaced_name(line, o, &n)
        }
        (0x1af4, 0x1050) => {
            let n = *b"virtio-gpu";
            push_spaced_name(line, o, &n)
        }
        (0x1af4, 0x1052) => {
            let n = *b"virtio-input";
            push_spaced_name(line, o, &n)
        }
        (0x1af4, 0x105a) => {
            let n = *b"virtio-fs";
            push_spaced_name(line, o, &n)
        }
        (0x1b36, 0x0001) => {
            let n = *b"QEMU PCI-PCI bridge";
            push_spaced_name(line, o, &n)
        }
        (0x1b36, 0x0002) => {
            let n = *b"QEMU QXL";
            push_spaced_name(line, o, &n)
        }
        (0x1b36, 0x0003) => {
            let n = *b"QEMU serial";
            push_spaced_name(line, o, &n)
        }
        (0x1b36, 0x0004) => {
            let n = *b"QEMU dual serial";
            push_spaced_name(line, o, &n)
        }
        (0x1b36, 0x0005) => {
            let n = *b"QEMU quad serial";
            push_spaced_name(line, o, &n)
        }
        (0x1b36, 0x0007) => {
            let n = *b"QEMU PCI test";
            push_spaced_name(line, o, &n)
        }
        (0x1b36, 0x0008) => {
            let n = *b"QEMU dual 16550A";
            push_spaced_name(line, o, &n)
        }
        (0x1b36, 0x0009) => {
            let n = *b"QEMU PCIe host";
            push_spaced_name(line, o, &n)
        }
        (0x1b36, 0x000a) => {
            let n = *b"QEMU PCIe root port";
            push_spaced_name(line, o, &n)
        }
        (0x1b36, 0x000b) => {
            let n = *b"QEMU SDHCI";
            push_spaced_name(line, o, &n)
        }
        (0x1b36, 0x000c) => {
            let n = *b"QEMU PVPanic PCI";
            push_spaced_name(line, o, &n)
        }
        (0x1b36, 0x000d) => {
            let n = *b"QEMU virtio-iommu";
            push_spaced_name(line, o, &n)
        }
        (0x1b36, 0x0010) => {
            let n = *b"QEMU NVMe";
            push_spaced_name(line, o, &n)
        }
        (0x1b36, 0x0011) => {
            let n = *b"QEMU PVPanic ISA";
            push_spaced_name(line, o, &n)
        }
        (0x1b36, 0x0013) => {
            let n = *b"QEMU mbox";
            push_spaced_name(line, o, &n)
        }
        (0x8086, 0x1237) => {
            let n = *b"i440FX host bridge";
            push_spaced_name(line, o, &n)
        }
        (0x8086, 0x7000) => {
            let n = *b"PIIX3 ISA";
            push_spaced_name(line, o, &n)
        }
        (0x8086, 0x7010) => {
            let n = *b"PIIX3 IDE";
            push_spaced_name(line, o, &n)
        }
        (0x8086, 0x7020) => {
            let n = *b"PIIX3 USB";
            push_spaced_name(line, o, &n)
        }
        (0x8086, 0x7110) => {
            let n = *b"PIIX4 ISA";
            push_spaced_name(line, o, &n)
        }
        (0x8086, 0x7111) => {
            let n = *b"PIIX4 IDE";
            push_spaced_name(line, o, &n)
        }
        (0x8086, 0x7113) => {
            let n = *b"PIIX4 ACPI";
            push_spaced_name(line, o, &n)
        }
        (0x8086, 0x29c0) => {
            let n = *b"Q35 host bridge";
            push_spaced_name(line, o, &n)
        }
        (0x8086, 0x2918) => {
            let n = *b"ICH9 LPC";
            push_spaced_name(line, o, &n)
        }
        (0x8086, 0x2922) => {
            let n = *b"ICH9 AHCI";
            push_spaced_name(line, o, &n)
        }
        (0x8086, 0x2930) => {
            let n = *b"ICH9 SMBus";
            push_spaced_name(line, o, &n)
        }
        (0x8086, 0x2415) => {
            let n = *b"ICH AC97";
            push_spaced_name(line, o, &n)
        }
        (0x8086, 0x2668) => {
            let n = *b"ICH6 HDA";
            push_spaced_name(line, o, &n)
        }
        (0x8086, 0x100e) => {
            let n = *b"e1000";
            push_spaced_name(line, o, &n)
        }
        (0x8086, 0x10d3) => {
            let n = *b"e1000e";
            push_spaced_name(line, o, &n)
        }
        (0x8086, 0x15d0) => {
            let n = *b"PCIe DRAM controller";
            push_spaced_name(line, o, &n)
        }
        (0x1234, 0x1111) => {
            let n = *b"Bochs VGA";
            push_spaced_name(line, o, &n)
        }
        (0x10ec, 0x8139) => {
            let n = *b"RTL8139";
            push_spaced_name(line, o, &n)
        }
        _ => o,
    }
}

fn push_class_name(line: &mut [u8], o: usize, class: u8, sub: u8) -> usize {
    match (class, sub) {
        (0x00, 0x00) => {
            let n = *b"Non-VGA unclassified";
            push_spaced_name(line, o, &n)
        }
        (0x00, 0x01) => {
            let n = *b"VGA unclassified";
            push_spaced_name(line, o, &n)
        }
        (0x01, 0x00) => {
            let n = *b"SCSI storage";
            push_spaced_name(line, o, &n)
        }
        (0x01, 0x01) => {
            let n = *b"IDE storage";
            push_spaced_name(line, o, &n)
        }
        (0x01, 0x04) => {
            let n = *b"RAID storage";
            push_spaced_name(line, o, &n)
        }
        (0x01, 0x05) => {
            let n = *b"ATA storage";
            push_spaced_name(line, o, &n)
        }
        (0x01, 0x06) => {
            let n = *b"SATA";
            push_spaced_name(line, o, &n)
        }
        (0x01, 0x07) => {
            let n = *b"SAS";
            push_spaced_name(line, o, &n)
        }
        (0x01, 0x08) => {
            let n = *b"NVMe";
            push_spaced_name(line, o, &n)
        }
        (0x01, _) => {
            let n = *b"Mass storage";
            push_spaced_name(line, o, &n)
        }
        (0x02, 0x00) => {
            let n = *b"Ethernet";
            push_spaced_name(line, o, &n)
        }
        (0x02, _) => {
            let n = *b"Network";
            push_spaced_name(line, o, &n)
        }
        (0x03, 0x00) => {
            let n = *b"VGA";
            push_spaced_name(line, o, &n)
        }
        (0x03, 0x02) => {
            let n = *b"3D controller";
            push_spaced_name(line, o, &n)
        }
        (0x03, _) => {
            let n = *b"Display";
            push_spaced_name(line, o, &n)
        }
        (0x04, 0x01) => {
            let n = *b"Audio";
            push_spaced_name(line, o, &n)
        }
        (0x04, 0x03) => {
            let n = *b"HD audio";
            push_spaced_name(line, o, &n)
        }
        (0x04, _) => {
            let n = *b"Multimedia";
            push_spaced_name(line, o, &n)
        }
        (0x05, _) => {
            let n = *b"Memory";
            push_spaced_name(line, o, &n)
        }
        (0x06, 0x00) => {
            let n = *b"Host bridge";
            push_spaced_name(line, o, &n)
        }
        (0x06, 0x01) => {
            let n = *b"ISA bridge";
            push_spaced_name(line, o, &n)
        }
        (0x06, 0x04) => {
            let n = *b"PCI-PCI bridge";
            push_spaced_name(line, o, &n)
        }
        (0x06, 0x09) => {
            let n = *b"PCI-PCI bridge (sub)";
            push_spaced_name(line, o, &n)
        }
        (0x06, _) => {
            let n = *b"Bridge";
            push_spaced_name(line, o, &n)
        }
        (0x07, 0x00) => {
            let n = *b"Serial";
            push_spaced_name(line, o, &n)
        }
        (0x07, _) => {
            let n = *b"Simple comm";
            push_spaced_name(line, o, &n)
        }
        (0x08, 0x05) => {
            let n = *b"SD host";
            push_spaced_name(line, o, &n)
        }
        (0x08, 0x80) => {
            let n = *b"System peripheral";
            push_spaced_name(line, o, &n)
        }
        (0x08, _) => {
            let n = *b"Base system";
            push_spaced_name(line, o, &n)
        }
        (0x09, _) => {
            let n = *b"Input";
            push_spaced_name(line, o, &n)
        }
        (0x0c, 0x03) => {
            let n = *b"USB";
            push_spaced_name(line, o, &n)
        }
        (0x0c, 0x05) => {
            let n = *b"SMBus";
            push_spaced_name(line, o, &n)
        }
        (0x0c, _) => {
            let n = *b"Serial bus";
            push_spaced_name(line, o, &n)
        }
        (0x0d, _) => {
            let n = *b"Wireless";
            push_spaced_name(line, o, &n)
        }
        (0xff, _) => {
            let n = *b"Unassigned";
            push_spaced_name(line, o, &n)
        }
        _ => o,
    }
}

fn format_dev(
    line: &mut [u8],
    bus: u8,
    slot: u8,
    func: u8,
    vend: u16,
    dev: u16,
    class: u8,
    sub: u8,
    prog: u8,
) -> usize {
    // "BB:SS.F VVVV:DDDD [name] CC:SS:PP [class-name]"
    let mut o = 0;
    o = hex_u8(line, o, bus);
    line[o] = b':';
    o += 1;
    o = hex_u8(line, o, slot);
    line[o] = b'.';
    o += 1;
    line[o] = b"0123456789abcdef"[func as usize];
    o += 1;
    line[o] = b' ';
    o += 1;
    o = hex_u16(line, o, vend);
    line[o] = b':';
    o += 1;
    o = hex_u16(line, o, dev);

    o = push_device_name(line, o, vend, dev);

    line[o] = b' ';
    o += 1;
    o = hex_u8(line, o, class);
    line[o] = b':';
    o += 1;
    o = hex_u8(line, o, sub);
    line[o] = b':';
    o += 1;
    o = hex_u8(line, o, prog);

    o = push_class_name(line, o, class, sub);
    o
}

fn format_count(msg: &mut [u8], n: usize) -> usize {
    let prefix = b"pci: ";
    msg[..prefix.len()].copy_from_slice(prefix);
    let mut o = prefix.len();
    let mut v = n;
    if v == 0 {
        msg[o] = b'0';
        o += 1;
    } else {
        let mut tmp = [0u8; 8];
        let mut t = 0;
        while v > 0 {
            tmp[t] = b'0' + (v % 10) as u8;
            v /= 10;
            t += 1;
        }
        while t > 0 {
            t -= 1;
            msg[o] = tmp[t];
            o += 1;
        }
    }
    let suf = b" devices";
    msg[o..o + suf.len()].copy_from_slice(suf);
    o + suf.len()
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
