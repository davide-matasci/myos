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

#![no_std]
#![no_main]

use myos_abi::{status_info, status_ok, status_warn, ABI_VERSION, KernelApi};

const MAX_DEV: usize = 64;
const BUF_CAP: usize = 8192;
const LINE_CAP: usize = 160;

static mut API: *const KernelApi = core::ptr::null();
static mut BUF: *mut u8 = core::ptr::null_mut();

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
        API = api;
        BUF = buf;

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
    unsafe {
        if data.is_null() && data_len != 0 {
            return -1;
        }
        let raw = if data_len == 0 {
            &[][..]
        } else {
            core::slice::from_raw_parts(data, data_len)
        };
        if !is_rescan_cmd(raw) {
            return -1;
        }
        let api = API;
        let buf = BUF;
        if api.is_null() || buf.is_null() {
            return -1;
        }
        let api_ref = &*api;
        let _found = publish(api_ref, buf);
        data_len as i32
    }
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

fn push_ascii(dst: &mut [u8], off: usize, s: &str) -> usize {
    let b = s.as_bytes();
    let n = b.len().min(dst.len().saturating_sub(off));
    dst[off..off + n].copy_from_slice(&b[..n]);
    off + n
}

fn device_name(vend: u16, dev: u16) -> Option<&'static str> {
    match (vend, dev) {
        // Red Hat / virtio (transitional 0x1000.. and modern 0x1040..)
        (0x1af4, 0x1000) | (0x1af4, 0x1041) => Some("virtio-net"),
        (0x1af4, 0x1001) | (0x1af4, 0x1042) => Some("virtio-blk"),
        (0x1af4, 0x1002) | (0x1af4, 0x1045) => Some("virtio-balloon"),
        (0x1af4, 0x1003) | (0x1af4, 0x1043) => Some("virtio-console"),
        (0x1af4, 0x1004) | (0x1af4, 0x1048) => Some("virtio-scsi"),
        (0x1af4, 0x1005) | (0x1af4, 0x1044) => Some("virtio-rng"),
        (0x1af4, 0x1009) | (0x1af4, 0x1049) => Some("virtio-9p"),
        (0x1af4, 0x1050) => Some("virtio-gpu"),
        (0x1af4, 0x1052) => Some("virtio-input"),
        (0x1af4, 0x105a) => Some("virtio-fs"),
        // QEMU devices (vendor 1b36)
        (0x1b36, 0x0001) => Some("QEMU PCI-PCI bridge"),
        (0x1b36, 0x0002) => Some("QEMU QXL"),
        (0x1b36, 0x0003) => Some("QEMU serial"),
        (0x1b36, 0x0004) => Some("QEMU dual serial"),
        (0x1b36, 0x0005) => Some("QEMU quad serial"),
        (0x1b36, 0x0007) => Some("QEMU PCI test"),
        (0x1b36, 0x0008) => Some("QEMU dual 16550A"),
        (0x1b36, 0x0009) => Some("QEMU PCIe host"),
        (0x1b36, 0x000a) => Some("QEMU PCIe root port"),
        (0x1b36, 0x000b) => Some("QEMU SDHCI"),
        (0x1b36, 0x000c) => Some("QEMU PVPanic PCI"),
        (0x1b36, 0x000d) => Some("QEMU virtio-iommu"),
        (0x1b36, 0x0010) => Some("QEMU NVMe"),
        (0x1b36, 0x0011) => Some("QEMU PVPanic ISA"),
        (0x1b36, 0x0013) => Some("QEMU mbox"),
        // Intel chipset pieces common under QEMU pc/q35
        (0x8086, 0x1237) => Some("i440FX host bridge"),
        (0x8086, 0x7000) => Some("PIIX3 ISA"),
        (0x8086, 0x7010) => Some("PIIX3 IDE"),
        (0x8086, 0x7020) => Some("PIIX3 USB"),
        (0x8086, 0x7110) => Some("PIIX4 ISA"),
        (0x8086, 0x7111) => Some("PIIX4 IDE"),
        (0x8086, 0x7113) => Some("PIIX4 ACPI"),
        (0x8086, 0x29c0) => Some("Q35 host bridge"),
        (0x8086, 0x2918) => Some("ICH9 LPC"),
        (0x8086, 0x2922) => Some("ICH9 AHCI"),
        (0x8086, 0x2930) => Some("ICH9 SMBus"),
        (0x8086, 0x2415) => Some("ICH AC97"),
        (0x8086, 0x2668) => Some("ICH6 HDA"),
        (0x8086, 0x100e) => Some("e1000"),
        (0x8086, 0x10d3) => Some("e1000e"),
        (0x8086, 0x15d0) => Some("PCIe DRAM controller"),
        // Bochs / QEMU VGA
        (0x1234, 0x1111) => Some("Bochs VGA"),
        // Realtek often used with -nic model=rtl8139
        (0x10ec, 0x8139) => Some("RTL8139"),
        _ => None,
    }
}

/// Class + subclass short names (PCI base-class codes). Prefer subclass
/// when known; otherwise fall back to the base class label.
fn class_name(class: u8, sub: u8) -> Option<&'static str> {
    match (class, sub) {
        (0x00, 0x00) => Some("Non-VGA unclassified"),
        (0x00, 0x01) => Some("VGA unclassified"),
        (0x01, 0x00) => Some("SCSI storage"),
        (0x01, 0x01) => Some("IDE storage"),
        (0x01, 0x04) => Some("RAID storage"),
        (0x01, 0x05) => Some("ATA storage"),
        (0x01, 0x06) => Some("SATA"),
        (0x01, 0x07) => Some("SAS"),
        (0x01, 0x08) => Some("NVMe"),
        (0x01, _) => Some("Mass storage"),
        (0x02, 0x00) => Some("Ethernet"),
        (0x02, _) => Some("Network"),
        (0x03, 0x00) => Some("VGA"),
        (0x03, 0x02) => Some("3D controller"),
        (0x03, _) => Some("Display"),
        (0x04, 0x01) => Some("Audio"),
        (0x04, 0x03) => Some("HD audio"),
        (0x04, _) => Some("Multimedia"),
        (0x05, _) => Some("Memory"),
        (0x06, 0x00) => Some("Host bridge"),
        (0x06, 0x01) => Some("ISA bridge"),
        (0x06, 0x04) => Some("PCI-PCI bridge"),
        (0x06, 0x09) => Some("PCI-PCI bridge (sub)"),
        (0x06, _) => Some("Bridge"),
        (0x07, 0x00) => Some("Serial"),
        (0x07, _) => Some("Simple comm"),
        (0x08, 0x05) => Some("SD host"),
        (0x08, 0x80) => Some("System peripheral"),
        (0x08, _) => Some("Base system"),
        (0x09, _) => Some("Input"),
        (0x0c, 0x03) => Some("USB"),
        (0x0c, 0x05) => Some("SMBus"),
        (0x0c, _) => Some("Serial bus"),
        (0x0d, _) => Some("Wireless"),
        (0xff, _) => Some("Unassigned"),
        _ => None,
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

    if let Some(name) = device_name(vend, dev) {
        line[o] = b' ';
        o += 1;
        o = push_ascii(line, o, name);
    }

    line[o] = b' ';
    o += 1;
    o = hex_u8(line, o, class);
    line[o] = b':';
    o += 1;
    o = hex_u8(line, o, sub);
    line[o] = b':';
    o += 1;
    o = hex_u8(line, o, prog);

    if let Some(cname) = class_name(class, sub) {
        line[o] = b' ';
        o += 1;
        o = push_ascii(line, o, cname);
    }
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
