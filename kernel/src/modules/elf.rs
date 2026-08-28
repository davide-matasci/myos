//! Minimal ELF64 ET_DYN / ET_EXEC loader.
//!
//! Handles `R_X86_64_RELATIVE` / `R_AARCH64_RELATIVE` plus the absolute
//! types rustc actually emits for a `no_std` PIE (`GLOB_DAT`, `JUMP_SLOT`,
//! `R_*_64`). Symbols come from `.dynsym` or `.symtab`.
//!
//! `image_span` / `realize` are shared by kernel modules (hello) and the
//! userspace init ELF. `load_bias` is the **runtime VA** slide
//! (`dest_va - min_v`); it may differ from the kernel buffer address when
//! staging into a heap copy that will later be mapped at `USER_BASE`.

use alloc::alloc::{alloc_zeroed, dealloc, Layout};
use myos_abi::{ModuleExit, ModuleInit};

const ELFMAG: [u8; 4] = [0x7f, b'E', b'L', b'F'];
const ELFCLASS64: u8 = 2;
const ELFDATA2LSB: u8 = 1;
const ET_EXEC: u16 = 2;
const ET_DYN: u16 = 3;
const PT_LOAD: u32 = 1;
const SHT_SYMTAB: u32 = 2;
const SHT_RELA: u32 = 4;
const SHT_REL: u32 = 9;
const SHT_DYNSYM: u32 = 11;
const SHN_UNDEF: u16 = 0;
const PF_X: u32 = 1;
const PF_W: u32 = 2;

const R_X86_64_NONE: u32 = 0;
const R_X86_64_64: u32 = 1;
const R_X86_64_GLOB_DAT: u32 = 6;
const R_X86_64_JUMP_SLOT: u32 = 7;
const R_X86_64_RELATIVE: u32 = 8;
const R_AARCH64_ABS64: u32 = 257;
const R_AARCH64_GLOB_DAT: u32 = 1025;
const R_AARCH64_JUMP_SLOT: u32 = 1026;
const R_AARCH64_RELATIVE: u32 = 1027;

#[cfg(target_arch = "x86_64")]
const EXPECT_MACHINE: u16 = 62; // EM_X86_64
#[cfg(target_arch = "aarch64")]
const EXPECT_MACHINE: u16 = 183; // EM_AARCH64

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
}

impl Loaded {
    pub unsafe fn free(self) {
        if !self.base.is_null() && self.size != 0 {
            if let Ok(layout) = Layout::from_size_align(self.size, PAGE) {
                unsafe { dealloc(self.base, layout) };
            }
        }
    }
}

struct Ehdr {
    e_entry: u64,
    e_phoff: usize,
    e_shoff: usize,
    e_phentsize: usize,
    e_phnum: usize,
    e_shentsize: usize,
    e_shnum: usize,
    min_v: u64,
    span: usize,
}

/// `(min_v, span, e_entry)` for the PT_LOAD image. `span` is `max_v - min_v`.
pub fn image_span(bytes: &[u8]) -> Result<(u64, usize, u64), LoadError> {
    let h = parse_ehdr(bytes)?;
    Ok((h.min_v, h.span, h.e_entry))
}

/// Copy `PT_LOAD` into `dest[vaddr - min_v]`, apply relocs with `load_bias`
/// as the runtime VA slide (`dest_va - min_v`), sync I-cache, return
/// `e_entry + load_bias`.
///
/// `dest` is only a kernel buffer; `load_bias` must be computed from the
/// address the image will actually run at (heap VA for modules, `USER_BASE`
/// for init), not from `dest as u64` unless those are the same.
pub unsafe fn realize(
    bytes: &[u8],
    dest: *mut u8,
    span: usize,
    load_bias: u64,
) -> Result<u64, LoadError> {
    let h = parse_ehdr(bytes)?;
    if h.span > span {
        return Err(LoadError::Unsupported);
    }
    for i in 0..h.e_phnum {
        let p = phdr_at(bytes, &h, i)?;
        if u32_at(bytes, p)? != PT_LOAD {
            continue;
        }
        let off = u64_at(bytes, p + 8)? as usize;
        let vaddr = u64_at(bytes, p + 16)?;
        let filesz = u64_at(bytes, p + 32)? as usize;
        if filesz == 0 {
            continue;
        }
        let src = bytes
            .get(off..off.checked_add(filesz).ok_or(LoadError::Truncated)?)
            .ok_or(LoadError::Truncated)?;
        let dest_off = (vaddr.wrapping_sub(h.min_v)) as usize;
        if dest_off
            .checked_add(filesz)
            .map(|e| e > span)
            .unwrap_or(true)
        {
            return Err(LoadError::Truncated);
        }
        unsafe {
            dest.add(dest_off)
                .copy_from_nonoverlapping(src.as_ptr(), filesz);
        }
    }
    apply_relocs(
        bytes,
        h.e_shoff,
        h.e_shentsize,
        h.e_shnum,
        load_bias,
        dest,
        span,
        h.min_v,
    )?;
    sync_icache(dest, span);
    Ok(h.e_entry.wrapping_add(load_bias))
}

/// Union of `PF_W` / `PF_X` on PT_LOAD covering `[page_va, page_va + 4K)`.
/// Uncovered pages are `(false, false)` (non-writable, non-executable).
pub fn page_perms(bytes: &[u8], page_va: u64) -> Result<(bool, bool), LoadError> {
    let h = parse_ehdr(bytes)?;
    let page_hi = page_va.saturating_add(PAGE as u64);
    let mut write = false;
    let mut exec = false;
    for i in 0..h.e_phnum {
        let p = phdr_at(bytes, &h, i)?;
        if u32_at(bytes, p)? != PT_LOAD {
            continue;
        }
        let flags = u32_at(bytes, p + 4)?;
        let vaddr = u64_at(bytes, p + 16)?;
        let memsz = u64_at(bytes, p + 40)?;
        let seg_hi = vaddr.saturating_add(memsz);
        if vaddr < page_hi && page_va < seg_hi {
            write |= flags & PF_W != 0;
            exec |= flags & PF_X != 0;
        }
    }
    Ok((write, exec))
}

pub fn load(bytes: &[u8]) -> Result<Loaded, LoadError> {
    let (min_v, span, _e_entry) = image_span(bytes)?;
    let layout = Layout::from_size_align(span.max(1), PAGE).map_err(|_| LoadError::Alloc)?;
    let base = unsafe { alloc_zeroed(layout) };
    if base.is_null() {
        return Err(LoadError::Alloc);
    }
    let load_bias = (base as u64).wrapping_sub(min_v);
    if let Err(e) = unsafe { realize(bytes, base, span, load_bias) } {
        unsafe { dealloc(base, layout) };
        return Err(e);
    }

    let h = match parse_ehdr(bytes) {
        Ok(h) => h,
        Err(e) => {
            unsafe { dealloc(base, layout) };
            return Err(e);
        }
    };
    let (init, exit) = match lookup_entry_points(
        bytes,
        h.e_shoff,
        h.e_shentsize,
        h.e_shnum,
        load_bias,
    ) {
        Ok(v) => v,
        Err(e) => {
            unsafe { dealloc(base, layout) };
            return Err(e);
        }
    };

    Ok(Loaded {
        base,
        size: span.max(1),
        init,
        exit,
    })
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

    let e_entry = u64_at(bytes, 24)?;
    let e_phoff = u64_at(bytes, 32)? as usize;
    let e_shoff = u64_at(bytes, 40)? as usize;
    let e_phentsize = u16_at(bytes, 54)? as usize;
    let e_phnum = u16_at(bytes, 56)? as usize;
    let e_shentsize = u16_at(bytes, 58)? as usize;
    let e_shnum = u16_at(bytes, 60)? as usize;
    if e_phentsize < 56 || e_shentsize < 64 {
        return Err(LoadError::Unsupported);
    }

    let mut min_v = u64::MAX;
    let mut max_v = 0u64;
    let mut nload = 0usize;
    for i in 0..e_phnum {
        let p = e_phoff
            .checked_add(i.checked_mul(e_phentsize).ok_or(LoadError::Truncated)?)
            .ok_or(LoadError::Truncated)?;
        if u32_at(bytes, p)? != PT_LOAD {
            continue;
        }
        let vaddr = u64_at(bytes, p + 16)?;
        let memsz = u64_at(bytes, p + 40)?;
        min_v = min_v.min(vaddr);
        max_v = max_v.max(vaddr.saturating_add(memsz));
        nload += 1;
    }
    if nload == 0 || min_v >= max_v {
        return Err(LoadError::NoLoad);
    }
    let span = (max_v - min_v) as usize;
    Ok(Ehdr {
        e_entry,
        e_phoff,
        e_shoff,
        e_phentsize,
        e_phnum,
        e_shentsize,
        e_shnum,
        min_v,
        span,
    })
}

fn phdr_at(bytes: &[u8], h: &Ehdr, i: usize) -> Result<usize, LoadError> {
    let _ = bytes;
    h.e_phoff
        .checked_add(i.checked_mul(h.e_phentsize).ok_or(LoadError::Truncated)?)
        .ok_or(LoadError::Truncated)
}

fn apply_relocs(
    bytes: &[u8],
    shoff: usize,
    shentsize: usize,
    shnum: usize,
    load_bias: u64,
    image: *mut u8,
    span: usize,
    min_v: u64,
) -> Result<(), LoadError> {
    for i in 0..shnum {
        let sh = shoff
            .checked_add(i.checked_mul(shentsize).ok_or(LoadError::Truncated)?)
            .ok_or(LoadError::Truncated)?;
        let sh_type = u32_at(bytes, sh + 4)?;
        let rela = match sh_type {
            SHT_RELA => true,
            SHT_REL => false,
            _ => continue,
        };
        let sh_flags = u64_at(bytes, sh + 8)?;
        if sh_flags & 0x2 == 0 {
            // Skip non-ALLOC reloc sections (debug).
            continue;
        }
        let sh_offset = u64_at(bytes, sh + 24)? as usize;
        let sh_size = u64_at(bytes, sh + 32)? as usize;
        let sh_link = u32_at(bytes, sh + 40)? as usize;
        let sh_entsize = u64_at(bytes, sh + 56)? as usize;
        if sh_entsize == 0 || sh_size == 0 {
            continue;
        }
        let n = sh_size / sh_entsize;
        let symtab_sh = if sh_link < shnum {
            Some(shoff + sh_link * shentsize)
        } else {
            None
        };
        for j in 0..n {
            let r = sh_offset
                .checked_add(j.checked_mul(shentsize_or(sh_entsize)?).ok_or(LoadError::Truncated)?)
                .ok_or(LoadError::Truncated)?;
            let r_offset = u64_at(bytes, r)?;
            let r_info = u64_at(bytes, r + 8)?;
            let r_type = (r_info & 0xffff_ffff) as u32;
            let r_sym = (r_info >> 32) as usize;
            let addend = if rela {
                u64_at(bytes, r + 16)? as i64
            } else {
                0
            };
            let loc_off = r_offset.wrapping_sub(min_v);
            if loc_off.saturating_add(8) > span as u64 {
                return Err(LoadError::BadReloc(r_type));
            }
            let loc = unsafe { image.add(loc_off as usize) as *mut u64 };
            match r_type {
                R_X86_64_NONE => {}
                R_X86_64_RELATIVE | R_AARCH64_RELATIVE => {
                    let a = if rela {
                        addend as u64
                    } else {
                        unsafe { loc.read_unaligned() }
                    };
                    unsafe { loc.write_unaligned(load_bias.wrapping_add(a)) };
                }
                R_X86_64_64 | R_X86_64_GLOB_DAT | R_X86_64_JUMP_SLOT | R_AARCH64_ABS64
                | R_AARCH64_GLOB_DAT | R_AARCH64_JUMP_SLOT => {
                    let s = symbol_value(bytes, symtab_sh, shoff, shentsize, r_sym, load_bias)?;
                    let val = if r_type == R_X86_64_64 || r_type == R_AARCH64_ABS64 {
                        let a = if rela {
                            addend as u64
                        } else {
                            unsafe { loc.read_unaligned() }
                        };
                        s.wrapping_add(a)
                    } else {
                        s
                    };
                    unsafe { loc.write_unaligned(val) };
                }
                other => return Err(LoadError::BadReloc(other)),
            }
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
    bytes: &[u8],
    symtab_sh: Option<usize>,
    shoff: usize,
    shentsize: usize,
    index: usize,
    load_bias: u64,
) -> Result<u64, LoadError> {
    let Some(sh) = symtab_sh else {
        return Err(LoadError::BadReloc(0));
    };
    let sh_offset = u64_at(bytes, sh + 24)? as usize;
    let sh_entsize = u64_at(bytes, sh + 56)? as usize;
    if sh_entsize < 24 {
        return Err(LoadError::Unsupported);
    }
    let s = sh_offset
        .checked_add(index.checked_mul(sh_entsize).ok_or(LoadError::Truncated)?)
        .ok_or(LoadError::Truncated)?;
    let st_shndx = u16_at(bytes, s + 6)?;
    let st_value = u64_at(bytes, s + 8)?;
    if st_shndx == SHN_UNDEF {
        return Err(LoadError::BadReloc(0));
    }
    let _ = (shoff, shentsize);
    Ok(load_bias.wrapping_add(st_value))
}

fn lookup_entry_points(
    bytes: &[u8],
    shoff: usize,
    shentsize: usize,
    shnum: usize,
    load_bias: u64,
) -> Result<(Option<ModuleInit>, Option<ModuleExit>), LoadError> {
    let mut init = None;
    let mut exit = None;
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
                _ => {}
            }
        }
    }
    Ok((init, exit))
}

fn cstr_at(bytes: &[u8], off: usize) -> Option<&[u8]> {
    let rest = bytes.get(off..)?;
    let end = rest.iter().position(|&b| b == 0)?;
    Some(&rest[..end])
}

fn sync_icache(start: *mut u8, size: usize) {
    #[cfg(target_arch = "aarch64")]
    unsafe {
        // Clean D-cache to PoU, invalidate I-cache; heap is normal WB RAM.
        let mut addr = start as usize & !63;
        let end = start as usize + size;
        while addr < end {
            core::arch::asm!("dc cvau, {x}", x = in(reg) addr, options(nostack));
            addr += 64;
        }
        core::arch::asm!("dsb ish", options(nostack));
        addr = start as usize & !63;
        while addr < end {
            core::arch::asm!("ic ivau, {x}", x = in(reg) addr, options(nostack));
            addr += 64;
        }
        core::arch::asm!("dsb ish; isb", options(nostack));
    }
    let _ = (start, size);
}

fn u16_at(b: &[u8], o: usize) -> Result<u16, LoadError> {
    let s = b.get(o..o + 2).ok_or(LoadError::Truncated)?;
    Ok(u16::from_le_bytes([s[0], s[1]]))
}

fn u32_at(b: &[u8], o: usize) -> Result<u32, LoadError> {
    let s = b.get(o..o + 4).ok_or(LoadError::Truncated)?;
    Ok(u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
}

fn u64_at(b: &[u8], o: usize) -> Result<u64, LoadError> {
    let s = b.get(o..o + 8).ok_or(LoadError::Truncated)?;
    Ok(u64::from_le_bytes([
        s[0], s[1], s[2], s[3], s[4], s[5], s[6], s[7],
    ]))
}
