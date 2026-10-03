//! `/dev/fb0`: the boot framebuffer for userspace, shaped like Linux fbdev.
//!
//! There is one framebuffer, the one Limine set up (VBE/GOP on x86_64,
//! `ramfb` on aarch64 and riscv64), at a fixed mode. A program reads its
//! geometry with `FBIOGET_VSCREENINFO` / `FBIOGET_FSCREENINFO` and maps it
//! with `mmap(MAP_SHARED)` (`user::do_mmap` maps these frames directly), or
//! reads and writes it like a file. Drawing does not stop the console module
//! painting text on the same pixels: `KDSETMODE KD_GRAPHICS` does (see
//! [`crate::console::set_graphics`]), until `KD_TEXT` or the owner's exit.

use crate::console;
use crate::fs::IoctlResult;
use crate::limine_boot;
use crate::task;
use crate::user;
use myos_abi::FramebufferInfo;

pub const FBIOGET_VSCREENINFO: usize = 0x4600;
pub const FBIOPUT_VSCREENINFO: usize = 0x4601;
pub const FBIOGET_FSCREENINFO: usize = 0x4602;
pub const FBIOPAN_DISPLAY: usize = 0x4606;
pub const FBIOBLANK: usize = 0x4611;

/// `ioctl(tty or fb, KDSETMODE, KD_TEXT | KD_GRAPHICS)`; `KDGETMODE` writes
/// the current mode as an int.
pub const KDSETMODE: usize = 0x4B3A;
pub const KDGETMODE: usize = 0x4B3B;
pub const KD_TEXT: usize = 0;
pub const KD_GRAPHICS: usize = 1;

/// `struct fb_var_screeninfo` and `struct fb_fix_screeninfo` (LP64).
const VAR_LEN: usize = 160;
const FIX_LEN: usize = 80;
const FB_TYPE_PACKED_PIXELS: u32 = 0;
const FB_VISUAL_TRUECOLOR: u32 = 2;

const PAGE: u64 = crate::user::PAGE as u64;

fn info() -> Option<FramebufferInfo> {
    console::framebuffer_info()
}

/// Whether there is a framebuffer to offer as `/dev/fb0`.
pub fn present() -> bool {
    info().is_some_and(|fb| phys_base(&fb).is_some())
}

/// Physical address of the first pixel. Limine hands out an HHDM address;
/// mapping it to userspace needs it page aligned.
fn phys_base(fb: &FramebufferInfo) -> Option<u64> {
    let phys = fb.addr.checked_sub(limine_boot::hhdm_offset())?;
    (phys % PAGE == 0).then_some(phys)
}

/// Bytes of pixel memory (`line_length * yres`).
pub fn len() -> usize {
    info().map_or(0, |fb| (fb.pitch * fb.height) as usize)
}

/// Frame `offset` bytes into the framebuffer, for a user mapping of `len`
/// bytes there. `None` if that runs past its last page.
pub fn frame_at(offset: usize, len: usize) -> Option<u64> {
    let fb = info()?;
    let base = phys_base(&fb)?;
    let span = len_pages() * PAGE as usize;
    (offset.checked_add(len)? <= span).then_some(base + offset as u64)
}

fn len_pages() -> usize {
    len().div_ceil(PAGE as usize)
}

/// Whether `phys` is framebuffer memory (a user mapping of it must not hand
/// the frame to the allocator on unmap, nor copy it on fork).
pub fn is_frame(phys: u64) -> bool {
    let Some(base) = info().as_ref().and_then(phys_base) else {
        return false;
    };
    phys >= base && phys < base + (len_pages() as u64) * PAGE
}

fn pixels() -> &'static mut [u8] {
    match info() {
        Some(fb) => unsafe { core::slice::from_raw_parts_mut(fb.addr as *mut u8, len()) },
        None => &mut [],
    }
}

/// `read` at `pos`: a copy of the pixels (a screenshot is `cat /dev/fb0`).
pub fn read(pos: usize, out: &mut [u8]) -> usize {
    let src = pixels();
    let n = out.len().min(src.len().saturating_sub(pos));
    if n != 0 {
        out[..n].copy_from_slice(&src[pos..pos + n]);
    }
    n
}

/// `write` at `pos`; stops at the end of the framebuffer.
pub fn write(pos: usize, buf: &[u8]) -> Option<usize> {
    let dst = pixels();
    let n = buf.len().min(dst.len().saturating_sub(pos));
    if n != 0 {
        dst[pos..pos + n].copy_from_slice(&buf[..n]);
    }
    Some(n)
}

fn put_u32(buf: &mut [u8], at: usize, v: u32) {
    buf[at..at + 4].copy_from_slice(&v.to_ne_bytes());
}

fn var_screeninfo(fb: &FramebufferInfo) -> [u8; VAR_LEN] {
    let mut v = [0u8; VAR_LEN];
    put_u32(&mut v, 0, fb.width as u32); // xres
    put_u32(&mut v, 4, fb.height as u32); // yres
    put_u32(&mut v, 8, fb.width as u32); // xres_virtual
    put_u32(&mut v, 12, fb.height as u32); // yres_virtual
    put_u32(&mut v, 24, fb.bpp as u32); // bits_per_pixel
    // red, green, blue: { offset, length, msb_right }; transp stays zero.
    put_u32(&mut v, 32, fb.r_shift as u32);
    put_u32(&mut v, 36, fb.r_size as u32);
    put_u32(&mut v, 44, fb.g_shift as u32);
    put_u32(&mut v, 48, fb.g_size as u32);
    put_u32(&mut v, 56, fb.b_shift as u32);
    put_u32(&mut v, 60, fb.b_size as u32);
    // height / width in mm: unknown.
    put_u32(&mut v, 88, u32::MAX);
    put_u32(&mut v, 92, u32::MAX);
    v
}

fn fix_screeninfo(fb: &FramebufferInfo) -> [u8; FIX_LEN] {
    let mut f = [0u8; FIX_LEN];
    f[..8].copy_from_slice(b"myos-fb\0");
    let start = phys_base(fb).unwrap_or(0);
    f[16..24].copy_from_slice(&start.to_ne_bytes()); // smem_start
    put_u32(&mut f, 24, len() as u32); // smem_len
    put_u32(&mut f, 28, FB_TYPE_PACKED_PIXELS);
    put_u32(&mut f, 36, FB_VISUAL_TRUECOLOR);
    put_u32(&mut f, 48, fb.pitch as u32); // line_length
    f
}

/// fbdev ioctls on `/dev/fb0` (the syscall layer routes them here because
/// they copy structs to and from userspace).
pub fn ioctl(request: usize, arg: usize) -> IoctlResult {
    let Some(fb) = info() else {
        return IoctlResult::Notty;
    };
    let aspace = task::current_aspace();
    match request {
        FBIOGET_VSCREENINFO => {
            if user::copy_to_user(aspace, arg, &var_screeninfo(&fb)) {
                IoctlResult::Ok
            } else {
                IoctlResult::Bad
            }
        }
        FBIOGET_FSCREENINFO => {
            if user::copy_to_user(aspace, arg, &fix_screeninfo(&fb)) {
                IoctlResult::Ok
            } else {
                IoctlResult::Bad
            }
        }
        // The mode is fixed: accept a request for the one we have.
        FBIOPUT_VSCREENINFO => {
            let mut v = [0u8; VAR_LEN];
            if !user::copy_from_user(aspace, arg, &mut v) {
                return IoctlResult::Bad;
            }
            if v[..8] == var_screeninfo(&fb)[..8] && v[24..28] == (fb.bpp as u32).to_ne_bytes() {
                IoctlResult::Ok
            } else {
                IoctlResult::Bad
            }
        }
        // No panning (the virtual size is the visible one) and no blanking.
        FBIOPAN_DISPLAY | FBIOBLANK => IoctlResult::Ok,
        _ => IoctlResult::Notty,
    }
}

/// `KDSETMODE` / `KDGETMODE`, on a console tty or on `/dev/fb0`.
pub fn kd_ioctl(request: usize, arg: usize) -> IoctlResult {
    match request {
        KDSETMODE => match arg {
            KD_GRAPHICS => {
                console::set_graphics(Some(task::current_pid()));
                IoctlResult::Ok
            }
            KD_TEXT => {
                console::set_graphics(None);
                IoctlResult::Ok
            }
            _ => IoctlResult::Bad,
        },
        KDGETMODE => {
            let mode = (if console::graphics() { KD_GRAPHICS } else { KD_TEXT }) as i32;
            if user::copy_to_user(task::current_aspace(), arg, &mode.to_ne_bytes()) {
                IoctlResult::Ok
            } else {
                IoctlResult::Bad
            }
        }
        _ => IoctlResult::Notty,
    }
}
