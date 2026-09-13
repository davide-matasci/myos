//! Full PCI configuration-space enumeration → `/proc/pci`.

#![no_std]
#![no_main]

use myos_abi::{status_info, status_ok, status_warn, ABI_VERSION, KernelApi};

const MAX_DEV: usize = 64;
const BUF_CAP: usize = 4096;

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

        let buf = (api.alloc)(BUF_CAP, 8);
        if buf.is_null() {
            status_warn(api, "pci_enum: alloc");
            return -3;
        }
        let out = core::slice::from_raw_parts_mut(buf, BUF_CAP);
        let mut len = 0usize;
        push_str(out, &mut len, "# bus:slot.func vendor:device class:sub:prog\n");

        let mut found = 0usize;
        // Cap buses: arch MAX_BUS differs; 32 is enough for QEMU virt / PC.
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
                    let mut line = [0u8; 80];
                    let n = format_dev(&mut line, bus, slot, func, vend, dev, class, sub, prog);
                    push_bytes(out, &mut len, &line[..n]);
                    push_str(out, &mut len, "\n");
                    found += 1;
                    if found >= MAX_DEV || len + 80 >= BUF_CAP {
                        break;
                    }
                }
                if found >= MAX_DEV || len + 80 >= BUF_CAP {
                    break;
                }
            }
            if found >= MAX_DEV || len + 80 >= BUF_CAP {
                break;
            }
        }

        let name = b"pci";
        let rc = (api.proc_register)(name.as_ptr(), name.len(), buf, len);
        if rc != 0 {
            status_warn(api, "pci_enum: proc_register");
            return -4;
        }
        let mut msg = [0u8; 32];
        let ml = format_count(&mut msg, found);
        status_ok(api, core::str::from_utf8(&msg[..ml]).unwrap_or("pci"));
        let _ = status_info;
        0
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
    // "BB:SS.F VVVV:DDDD CC:SS:PP"
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
    line[o] = b' ';
    o += 1;
    o = hex_u8(line, o, class);
    line[o] = b':';
    o += 1;
    o = hex_u8(line, o, sub);
    line[o] = b':';
    o += 1;
    o = hex_u8(line, o, prog);
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
