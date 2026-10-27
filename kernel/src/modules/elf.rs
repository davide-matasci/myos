//! Minimal ELF64 ET_DYN / ET_EXEC loader.
//!
//! Handles `R_X86_64_RELATIVE` / `R_AARCH64_RELATIVE` plus the absolute
//! types rustc actually emits for a `no_std` PIE (`GLOB_DAT`, `JUMP_SLOT`,
//! `R_*_64`). Symbols come from `.dynsym` or `.symtab`.
//!
//! `image_span` / `realize_as` also load the userspace images: copy PT_LOAD
//! into a caller buffer and apply relocs for a chosen load bias (0 for an
//! `ET_EXEC` image, which loads at its link address), with no
//! `module_init`.

use alloc::alloc::{alloc_zeroed, dealloc, Layout};
use alloc::borrow::Cow;
use myos_abi::{ModuleExit, ModuleInit, ModuleRescan};

const ELFMAG: [u8; 4] = [0x7f, b'E', b'L', b'F'];
const ELFCLASS64: u8 = 2;
const ELFDATA2LSB: u8 = 1;
const ET_EXEC: u16 = 2;
const ET_DYN: u16 = 3;
const PT_LOAD: u32 = 1;
const PT_INTERP: u32 = 3;
const PF_X: u32 = 1;
const PF_W: u32 = 2;
const PF_R: u32 = 4;
const SHT_SYMTAB: u32 = 2;
const SHT_RELA: u32 = 4;
const SHT_REL: u32 = 9;
const SHT_DYNSYM: u32 = 11;
const SHN_UNDEF: u16 = 0;

// Dynamic relocation types for this kernel's arch. The numbers overlap
// across arches (2 is R_X86_64_PC32 but R_RISCV_64), so only the native set
// is accepted; anything else is `BadReloc`.
const R_NONE: u32 = 0;
use crate::arch::elf::{EXPECT_MACHINE, R_ABS64, R_GLOB_DAT, R_JUMP_SLOT, R_RELATIVE};

const PAGE: usize = 4096;

#[derive(Debug)]
pub enum LoadError {
    Truncated,
    NotElf,
    Unsupported,
    BadMachine,
    NoLoad,
    Alloc,
    BadReloc(u32),
    MissingInit,
    InitFailed(i32),
}

impl core::fmt::Display for LoadError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Truncated => write!(f, "truncated ELF"),
            Self::NotElf => write!(f, "not ELF"),
            Self::Unsupported => write!(f, "unsupported ELF"),
            Self::BadMachine => write!(f, "wrong machine"),
            Self::NoLoad => write!(f, "no PT_LOAD"),
            Self::Alloc => write!(f, "out of memory"),
            Self::BadReloc(t) => write!(f, "reloc type {t}"),
            Self::MissingInit => write!(f, "no module_init"),
            Self::InitFailed(c) => write!(f, "module_init returned {c}"),
        }
    }
}

pub struct Loaded {
    pub base: *mut u8,
    pub size: usize,
    pub init: Option<ModuleInit>,
    pub exit: Option<ModuleExit>,
    pub rescan: Option<ModuleRescan>,
}

impl Loaded {
    pub unsafe fn free(self) {
        unsafe { free_image(self.base, self.size) }
    }
}

/// Release a loaded image (`rmmod`, or a module whose init failed).
pub unsafe fn free_image(base: *mut u8, size: usize) {
    if !base.is_null() && size != 0 {
        if let Ok(layout) = Layout::from_size_align(size, PAGE) {
            unsafe { dealloc(base, layout) };
        }
    }
}

/// PT_LOAD span and `e_entry` as stored in the file (not biased).
#[derive(Clone, Copy)]
pub struct ImageSpan {
    /// The start of the page the lowest PT_LOAD begins in.
    pub min_vaddr: u64,
    pub span: usize,
    pub entry: u64,
}

struct Ehdr {
    e_type: u16,
    e_entry: u64,
    e_phoff: usize,
    e_shoff: usize,
    e_phentsize: usize,
    e_phnum: usize,
    e_shentsize: usize,
    e_shnum: usize,
}

fn parse_ehdr(bytes: &[u8]) -> Result<Ehdr, LoadError> {
    if bytes.len() < 64 {
        return Err(LoadError::Truncated);
    }
    if bytes[0..4] != ELFMAG {
        return Err(LoadError::NotElf);
    }
    if bytes[4] != ELFCLASS64 || bytes[5] != ELFDATA2LSB || bytes[6] != 1 {
        return Err(LoadError::Unsupported);
    }
    let e_type = u16_at(bytes, 16)?;
    let e_machine = u16_at(bytes, 18)?;
    if e_type != ET_DYN && e_type != ET_EXEC {
        return Err(LoadError::Unsupported);
    }
    if e_machine != EXPECT_MACHINE {
        return Err(LoadError::BadMachine);
    }
    let e_phentsize = u16_at(bytes, 54)? as usize;
    let e_shentsize = u16_at(bytes, 58)? as usize;
    if e_phentsize < 56 || e_shentsize < 64 {
        return Err(LoadError::Unsupported);
    }
    Ok(Ehdr {
        e_type,
        e_entry: u64_at(bytes, 24)?,
        e_phoff: u64_at(bytes, 32)? as usize,
        e_shoff: u64_at(bytes, 40)? as usize,
        e_phentsize,
        e_phnum: u16_at(bytes, 56)? as usize,
        e_shentsize,
        e_shnum: u16_at(bytes, 60)? as usize,
    })
}

/// One PT_LOAD segment (file vaddrs, `p_flags`).
#[derive(Clone, Copy, Debug)]
pub struct LoadSegment {
    pub vaddr: u64,
    pub memsz: u64,
    pub flags: u32,
    /// Where its bytes are in the file, and how many (`filesz <= memsz`).
    pub offset: u64,
    pub filesz: u64,
}

/// Visit each PT_LOAD in `bytes`. Returns the number of segments walked.
pub fn for_each_load_segment(
    bytes: &[u8],
    mut f: impl FnMut(LoadSegment),
) -> Result<usize, LoadError> {
    let hdr = parse_ehdr(bytes)?;
    let mut n = 0usize;
    for i in 0..hdr.e_phnum {
        let p = hdr
            .e_phoff
            .checked_add(i.checked_mul(hdr.e_phentsize).ok_or(LoadError::Truncated)?)
            .ok_or(LoadError::Truncated)?;
        if u32_at(bytes, p)? != PT_LOAD {
            continue;
        }
        let flags = u32_at(bytes, p + 4)?;
        let offset = u64_at(bytes, p + 8)?;
        let vaddr = u64_at(bytes, p + 16)?;
        let filesz = u64_at(bytes, p + 32)?;
        let memsz = u64_at(bytes, p + 40)?;
        f(LoadSegment {
            vaddr,
            memsz,
            flags,
            offset,
            filesz,
        });
        n += 1;
    }
    Ok(n)
}

pub fn pf_to_prot(flags: u32) -> usize {
    let mut prot = 0usize;
    if flags & PF_R != 0 {
        prot |= 1; // PROT_READ
    }
    if flags & PF_W != 0 {
        prot |= 2; // PROT_WRITE
    }
    if flags & PF_X != 0 {
        prot |= 4; // PROT_EXEC
    }
    if prot == 0 {
        prot = 1;
    }
    prot
}

pub fn image_span(bytes: &[u8]) -> Result<ImageSpan, LoadError> {
    let hdr = parse_ehdr(bytes)?;
    span_from(&hdr, bytes)
}

/// The highest virtual address a loadable image may reach. Real images (PIE
/// at 0, `ET_EXEC` at a low link address, dynamic linkers, kernel modules) sit far
/// below this; a crafted ELF claiming an address near the top of the 64-bit
/// space is rejected here so the loaders' later `vaddr + memsz`, page round-up
/// and `load_addr - min_vaddr` arithmetic cannot overflow or underflow.
const IMAGE_VADDR_MAX: u64 = 1 << 46;

fn span_from(hdr: &Ehdr, bytes: &[u8]) -> Result<ImageSpan, LoadError> {
    let mut min_v = u64::MAX;
    let mut max_v = 0u64;
    let mut nload = 0usize;
    for i in 0..hdr.e_phnum {
        let p = hdr
            .e_phoff
            .checked_add(i.checked_mul(hdr.e_phentsize).ok_or(LoadError::Truncated)?)
            .ok_or(LoadError::Truncated)?;
        if u32_at(bytes, p)? != PT_LOAD {
            continue;
        }
        let vaddr = u64_at(bytes, p + 16)?;
        let filesz = u64_at(bytes, p + 32)?;
        let memsz = u64_at(bytes, p + 40)?;
        // A malformed segment — one whose file size exceeds its memory size
        // (realize would copy past the image buffer), whose end wraps, or
        // whose end sits beyond the address space a real image uses — is
        // rejected here, the one place every loader passes through.
        if filesz > memsz {
            return Err(LoadError::Unsupported);
        }
        let end = vaddr.checked_add(memsz).ok_or(LoadError::Unsupported)?;
        if end > IMAGE_VADDR_MAX {
            return Err(LoadError::Unsupported);
        }
        min_v = min_v.min(vaddr);
        max_v = max_v.max(end);
        nload += 1;
    }
    if nload == 0 || min_v >= max_v {
        return Err(LoadError::NoLoad);
    }
    // From the page the lowest segment starts in: an image is mapped by
    // pages, and an `ET_EXEC` one at its link address (bias 0) even when its
    // first segment does not start a page.
    min_v &= !(PAGE as u64 - 1);
    Ok(ImageSpan {
        min_vaddr: min_v,
        span: (max_v - min_v) as usize,
        entry: hdr.e_entry,
    })
}

/// Copy PT_LOAD into `dest` (`image_span().span` bytes) and apply relocs
/// so pointers become `load_bias + vaddr`. Returns the biased entry.
///
/// With `relocate == false` only PT_LOAD is copied: for an image that a
/// dynamic linker relocates (itself, or a dynamically linked program it
/// starts), against symbols the kernel cannot resolve.
pub fn realize_as(bytes: &[u8], dest: *mut u8, load_bias: u64, relocate: bool) -> Result<u64, LoadError> {
    let hdr = parse_ehdr(bytes)?;
    let info = span_from(&hdr, bytes)?;
    realize_into(bytes, &hdr, &info, dest, load_bias, relocate)?;
    Ok(info.entry.wrapping_add(load_bias))
}

/// The `PT_INTERP` path (without its NUL), for a dynamically linked image.
/// Where `PT_INTERP`'s path is in the file (offset, length), from the
/// headers alone (`bytes` need not hold the path itself).
pub fn interp_range(bytes: &[u8]) -> Option<(usize, usize)> {
    let hdr = parse_ehdr(bytes).ok()?;
    for i in 0..hdr.e_phnum {
        let p = hdr.e_phoff.checked_add(i.checked_mul(hdr.e_phentsize)?)?;
        if u32_at(bytes, p).ok()? == PT_INTERP {
            return Some((u64_at(bytes, p + 8).ok()? as usize, u64_at(bytes, p + 32).ok()? as usize));
        }
    }
    None
}

/// The image of a native program `exec` maps from its file instead of
/// reading it whole (one too large to be): a position-independent ELF of
/// one empty page. The program's segments are in the mmap window, as a
/// mapped Linux program's are; the image only places the heap and stack.
pub static EMPTY_IMAGE: [u8; 120] = empty_image();

const fn empty_image() -> [u8; 120] {
    let mut b = [0u8; 120];
    b = put_le(b, 0, u32::from_le_bytes(ELFMAG) as u64, 4);
    b = put_le(b, 4, (ELFCLASS64 as u64) | (ELFDATA2LSB as u64) << 8 | 1 << 16, 3);
    b = put_le(b, 16, ET_DYN as u64, 2);
    b = put_le(b, 18, EXPECT_MACHINE as u64, 2);
    b = put_le(b, 20, 1, 4); // e_version
    b = put_le(b, 32, 64, 8); // e_phoff
    b = put_le(b, 52, 64, 2); // e_ehsize
    b = put_le(b, 54, 56, 2); // e_phentsize
    b = put_le(b, 56, 1, 2); // e_phnum
    b = put_le(b, 58, 64, 2); // e_shentsize
    b = put_le(b, 64, PT_LOAD as u64, 4);
    b = put_le(b, 68, PF_R as u64, 4);
    b = put_le(b, 96, 120, 8); // p_filesz: the headers
    b = put_le(b, 104, 4096, 8); // p_memsz
    put_le(b, 112, 4096, 8) // p_align
}

/// `b` with the `n` low bytes of `v` at `at`, little-endian.
const fn put_le(mut b: [u8; 120], at: usize, v: u64, n: usize) -> [u8; 120] {
    let mut i = 0;
    while i < n {
        b[at + i] = (v >> (8 * i)) as u8;
        i += 1;
    }
    b
}

/// Whether `bytes` starts a position-independent ELF (`ET_DYN`).
pub fn is_pie(bytes: &[u8]) -> bool {
    parse_ehdr(bytes).map_or(false, |h| h.e_type == ET_DYN)
}

pub fn interp_path(bytes: &[u8]) -> Option<&[u8]> {
    let hdr = parse_ehdr(bytes).ok()?;
    for i in 0..hdr.e_phnum {
        let p = hdr.e_phoff.checked_add(i.checked_mul(hdr.e_phentsize)?)?;
        if u32_at(bytes, p).ok()? != PT_INTERP {
            continue;
        }
        let off = u64_at(bytes, p + 8).ok()? as usize;
        let len = u64_at(bytes, p + 32).ok()? as usize;
        let s = bytes.get(off..off.checked_add(len)?)?;
        return Some(s.split(|&b| b == 0).next().unwrap_or(s));
    }
    None
}

fn realize_into(
    bytes: &[u8],
    hdr: &Ehdr,
    info: &ImageSpan,
    dest: *mut u8,
    load_bias: u64,
    relocate: bool,
) -> Result<(), LoadError> {
    unsafe { dest.write_bytes(0, info.span) };
    for i in 0..hdr.e_phnum {
        let p = hdr.e_phoff + i * hdr.e_phentsize;
        if u32_at(bytes, p)? != PT_LOAD {
            continue;
        }
        let off = u64_at(bytes, p + 8)? as usize;
        let vaddr = u64_at(bytes, p + 16)?;
        let filesz = u64_at(bytes, p + 32)? as usize;
        let src = bytes
            .get(off..off.checked_add(filesz).ok_or(LoadError::Truncated)?)
            .ok_or(LoadError::Truncated)?;
        let d = unsafe { dest.add((vaddr - info.min_vaddr) as usize) };
        unsafe { d.copy_from_nonoverlapping(src.as_ptr(), filesz) };
    }
    if relocate {
        let img = dest;
        let span = info.span;
        apply_relocs(
            &bytes,
            hdr.e_shoff,
            hdr.e_shentsize,
            hdr.e_shnum,
            span,
            info.min_vaddr,
            load_bias,
            &mut |off, f| {
                // In the image: apply_relocs checked `off + 8 <= span`.
                let loc = unsafe { img.add(off) } as *mut u64;
                unsafe { loc.write_unaligned(f(loc.read_unaligned())) };
                true
            },
        )?;
    }
    sync_icache(dest, info.span);
    Ok(())
}

pub fn load(bytes: &[u8]) -> Result<Loaded, LoadError> {
    let hdr = parse_ehdr(bytes)?;
    let info = span_from(&hdr, bytes)?;
    let layout = Layout::from_size_align(info.span.max(1), PAGE).map_err(|_| LoadError::Alloc)?;
    let base = unsafe { alloc_zeroed(layout) };
    if base.is_null() {
        return Err(LoadError::Alloc);
    }
    let load_bias = base as u64 - info.min_vaddr;
    if let Err(e) = realize_into(bytes, &hdr, &info, base, load_bias, true) {
        unsafe { dealloc(base, layout) };
        return Err(e);
    }
    let (init, exit, rescan) = lookup_entry_points(
        bytes,
        hdr.e_shoff,
        hdr.e_shentsize,
        hdr.e_shnum,
        load_bias,
    )?;
    Ok(Loaded {
        base,
        size: info.span.max(1),
        init,
        exit,
        rescan,
    })
}

/// An ELF file as [`relocate`] reads it: `len` bytes at `off`, borrowed
/// from the whole file in memory or read from a file piecewise (a program
/// `exec` maps instead of reading it whole).
pub trait FileSrc {
    fn at(&self, off: usize, len: usize) -> Result<Cow<'_, [u8]>, LoadError>;
}

impl FileSrc for &[u8] {
    fn at(&self, off: usize, len: usize) -> Result<Cow<'_, [u8]>, LoadError> {
        let end = off.checked_add(len).ok_or(LoadError::Truncated)?;
        self.get(off..end).map(Cow::Borrowed).ok_or(LoadError::Truncated)
    }
}

/// Relocations read per [`FileSrc::at`] call.
const RELOCS_PER_READ: usize = 4096;

/// Apply the relocations of the image `src` holds the file of, loaded with
/// `load_bias`: `store(off, f)` replaces the word `off` bytes into the
/// image (from `min_v`) by `f` of it, false if it cannot.
pub fn relocate(
    src: &dyn FileSrc,
    load_bias: u64,
    store: &mut dyn FnMut(usize, &dyn Fn(u64) -> u64) -> bool,
) -> Result<(), LoadError> {
    let hdr = {
        let head = src.at(0, 64)?;
        parse_ehdr(&head)?
    };
    let info = {
        let phdrs = src.at(0, hdr.e_phoff.saturating_add(hdr.e_phnum.saturating_mul(hdr.e_phentsize)))?;
        span_from(&hdr, &phdrs)?
    };
    apply_relocs(src, hdr.e_shoff, hdr.e_shentsize, hdr.e_shnum, info.span, info.min_vaddr, load_bias, store)
}

fn apply_relocs(
    src: &dyn FileSrc,
    shoff: usize,
    shentsize: usize,
    shnum: usize,
    span: usize,
    min_v: u64,
    load_bias: u64,
    store: &mut dyn FnMut(usize, &dyn Fn(u64) -> u64) -> bool,
) -> Result<(), LoadError> {
    let shdrs = src.at(shoff, shnum.checked_mul(shentsize).ok_or(LoadError::Truncated)?)?;
    for i in 0..shnum {
        let sh = i * shentsize;
        let sh_type = u32_at(&shdrs, sh + 4)?;
        let rela = match sh_type {
            SHT_RELA => true,
            SHT_REL => false,
            _ => continue,
        };
        let sh_flags = u64_at(&shdrs, sh + 8)?;
        let sh_link = u32_at(&shdrs, sh + 40)? as usize;
        // .rela.dyn is often non-ALLOC. Skipping it left GOT entries at
        // link addresses, so user/ok MSG_BUF failed user_range_ok (#97/#98).
        // Still skip debug relocs (non-ALLOC, linked to SHT_SYMTAB).
        if sh_flags & 0x2 == 0 {
            let dynsym = sh_link < shnum && u32_at(&shdrs, sh_link * shentsize + 4)? == SHT_DYNSYM;
            if !dynsym {
                continue;
            }
        }
        let sh_offset = u64_at(&shdrs, sh + 24)? as usize;
        let sh_size = u64_at(&shdrs, sh + 32)? as usize;
        let sh_entsize = u64_at(&shdrs, sh + 56)? as usize;
        if sh_entsize == 0 || sh_size == 0 {
            continue;
        }
        let n = sh_size / sh_entsize;
        let symtab_sh = (sh_link < shnum).then(|| sh_link * shentsize);
        let mut j = 0;
        while j < n {
            let k = (n - j).min(RELOCS_PER_READ);
            let at = j
                .checked_mul(shentsize_or(sh_entsize)?)
                .and_then(|o| sh_offset.checked_add(o))
                .ok_or(LoadError::Truncated)?;
            let relocs = src.at(at, k * sh_entsize)?;
            for r in (0..k).map(|i| i * sh_entsize) {
                let r_offset = u64_at(&relocs, r)?;
                let r_info = u64_at(&relocs, r + 8)?;
                let r_type = (r_info & 0xffff_ffff) as u32;
                let r_sym = (r_info >> 32) as usize;
                let addend = if rela { Some(u64_at(&relocs, r + 16)?) } else { None };
                // The addend: the RELA entry's, or the word itself (REL).
                let off = r_offset.wrapping_sub(min_v) as usize;
                if r_offset < min_v || off.saturating_add(8) > span {
                    return Err(LoadError::BadReloc(r_type));
                }
                let ok = match r_type {
                    R_NONE => true,
                    R_RELATIVE => store(off, &|w| load_bias.wrapping_add(addend.unwrap_or(w))),
                    R_ABS64 | R_GLOB_DAT | R_JUMP_SLOT => {
                        let s = symbol_value(src, &shdrs, symtab_sh, r_sym, load_bias)?;
                        if r_type == R_ABS64 {
                            store(off, &|w| s.wrapping_add(addend.unwrap_or(w)))
                        } else {
                            store(off, &|_| s)
                        }
                    }
                    other => return Err(LoadError::BadReloc(other)),
                };
                if !ok {
                    return Err(LoadError::BadReloc(r_type));
                }
            }
            j += k;
        }
    }
    Ok(())
}

fn shentsize_or(n: usize) -> Result<usize, LoadError> {
    if n == 0 {
        Err(LoadError::Unsupported)
    } else {
        Ok(n)
    }
}

fn symbol_value(
    src: &dyn FileSrc,
    shdrs: &[u8],
    symtab_sh: Option<usize>,
    index: usize,
    load_bias: u64,
) -> Result<u64, LoadError> {
    let Some(sh) = symtab_sh else {
        return Err(LoadError::BadReloc(0));
    };
    let sh_offset = u64_at(shdrs, sh + 24)? as usize;
    let sh_entsize = u64_at(shdrs, sh + 56)? as usize;
    if sh_entsize < 24 {
        return Err(LoadError::Unsupported);
    }
    let s = sh_offset
        .checked_add(index.checked_mul(sh_entsize).ok_or(LoadError::Truncated)?)
        .ok_or(LoadError::Truncated)?;
    let sym = src.at(s, 24)?;
    let st_shndx = u16_at(&sym, 6)?;
    let st_value = u64_at(&sym, 8)?;
    if st_shndx == SHN_UNDEF {
        return Err(LoadError::BadReloc(0));
    }
    Ok(load_bias.wrapping_add(st_value))
}

fn lookup_entry_points(
    bytes: &[u8],
    shoff: usize,
    shentsize: usize,
    shnum: usize,
    load_bias: u64,
) -> Result<(Option<ModuleInit>, Option<ModuleExit>, Option<ModuleRescan>), LoadError> {
    let mut init = None;
    let mut exit = None;
    let mut rescan = None;
    for i in 0..shnum {
        let sh = shoff + i * shentsize;
        let sh_type = u32_at(bytes, sh + 4)?;
        if sh_type != SHT_SYMTAB && sh_type != SHT_DYNSYM {
            continue;
        }
        let sh_offset = u64_at(bytes, sh + 24)? as usize;
        let sh_size = u64_at(bytes, sh + 32)? as usize;
        let sh_link = u32_at(bytes, sh + 40)? as usize;
        let sh_entsize = u64_at(bytes, sh + 56)? as usize;
        if sh_entsize < 24 || sh_link >= shnum {
            continue;
        }
        let str_sh = shoff + sh_link * shentsize;
        let str_off = u64_at(bytes, str_sh + 24)? as usize;
        let str_size = u64_at(bytes, str_sh + 32)? as usize;
        let strs = bytes.get(str_off..str_off.saturating_add(str_size)).unwrap_or(&[]);
        let n = sh_size / sh_entsize;
        for j in 0..n {
            let s = sh_offset + j * sh_entsize;
            let st_name = u32_at(bytes, s)? as usize;
            let st_value = u64_at(bytes, s + 8)?;
            if st_name == 0 {
                continue;
            }
            let name = cstr_at(strs, st_name);
            let addr = (load_bias + st_value) as usize;
            match name {
                Some(b"module_init") => {
                    init = Some(unsafe { core::mem::transmute::<usize, ModuleInit>(addr) });
                }
                Some(b"module_exit") => {
                    exit = Some(unsafe { core::mem::transmute::<usize, ModuleExit>(addr) });
                }
                Some(b"module_rescan") => {
                    rescan = Some(unsafe { core::mem::transmute::<usize, ModuleRescan>(addr) });
                }
                _ => {}
            }
        }
    }
    Ok((init, exit, rescan))
}

fn cstr_at(bytes: &[u8], off: usize) -> Option<&[u8]> {
    let rest = bytes.get(off..)?;
    let end = rest.iter().position(|&b| b == 0)?;
    Some(&rest[..end])
}

fn sync_icache(start: *mut u8, size: usize) {
    crate::arch::sync_icache(start as usize, size);
}

// An offset from the file near `usize::MAX` must not overflow the end of
// the range (a hostile header): such a field is just past the end.
fn u16_at(b: &[u8], o: usize) -> Result<u16, LoadError> {
    let s = o.checked_add(2).and_then(|e| b.get(o..e)).ok_or(LoadError::Truncated)?;
    Ok(u16::from_le_bytes([s[0], s[1]]))
}
fn u32_at(b: &[u8], o: usize) -> Result<u32, LoadError> {
    let s = o.checked_add(4).and_then(|e| b.get(o..e)).ok_or(LoadError::Truncated)?;
    Ok(u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
}
fn u64_at(b: &[u8], o: usize) -> Result<u64, LoadError> {
    let s = o.checked_add(8).and_then(|e| b.get(o..e)).ok_or(LoadError::Truncated)?;
    Ok(u64::from_le_bytes([
        s[0], s[1], s[2], s[3], s[4], s[5], s[6], s[7],
    ]))
}
