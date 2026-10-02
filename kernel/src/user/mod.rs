//! Usermode: nested `user/init` ELF, per-process page tables, syscalls.

mod aspace;
mod enter;
mod image;
mod syscall;
mod uaccess;
pub use aspace::*;
pub use enter::*;
use image::*;
pub use image::AuxV;
pub use syscall::*;
pub use uaccess::*;

use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use spin::Mutex;

use crate::fs;
use crate::mm;
use crate::modules::elf;
use crate::task;

/// Linux mmap prot/flags (newlib + tcc).
const PROT_READ: usize = 1;
const PROT_WRITE: usize = 2;
const PROT_EXEC: usize = 4;
const MAP_PRIVATE: usize = 0x02;
const MAP_FIXED: usize = 0x10;
const MAP_ANON: usize = 0x20;

pub use crate::arch::{DEFAULT_USER_BASE, HEAP_PAGES, MMAP_AREA_PAGES, USER_STACK_PAGES};
pub const PAGE: usize = 4096;
/// Cap for fresh `load_user_elf` (init + typical programs) and on-stack frame arrays.
/// Keep modest: bumping this also sizes `[u64; N]` on the task stack and used to
/// force `elf_scratch_mut` to grab N contiguous frames before init could run.
const MAX_INIT_PAGES: usize = 1024;
/// Cap for in-place `expand_user_elf` of larger bootfs ELFs (uutils / ripgrep / git).
/// Must stay within QEMU RAM given leaked post-exec frames (x86 CI is 1024 MiB).
/// Full feat_common_core (~2.4k pages) OOMed; ship a smaller multicall instead.
/// Phase-1 git static-pie spans ~1080 pages (BSS included); keep ≤1152 so
/// aarch64 image+stack+heap stays within the 4×512 L2 spill cap (2048 pages).
const MAX_EXPAND_PAGES: usize = 1152;
/// Largest image we may map, fork-copy, or stage in ELF scratch.
const MAX_ELF_PAGES: usize = if MAX_EXPAND_PAGES > MAX_INIT_PAGES {
    MAX_EXPAND_PAGES
} else {
    MAX_INIT_PAGES
};

/// In-place `reload_user_elf` scratch and mapping cap (sbase-cat scale).
const MAX_RELOAD_PAGES: usize = 40;
/// Minimum code pages reserved below the user stack so post-fork `exec` can
/// `reload_user_elf` the largest newlib/sbase ELFs (today `sbase-cat`).
const USER_EXEC_RELOAD_PAGES: usize = 36;
const MAX_PATH: usize = 256;
/// exec argument and environment limits: at most this many strings each, and
/// this many bytes for all of them together (NULs included), which also bounds
/// what the new image's stack gives up to them.
pub(crate) const MAX_ARGC: usize = 1024;
pub(crate) const MAX_ENVC: usize = 1024;
pub(crate) const MAX_EXEC_STRINGS: usize = 128 * 1024;
const SYSERR: usize = usize::MAX;
/// open(2) of a FIFO for writing with O_NONBLOCK and no reader (ENXIO).
const SYSERR_ENXIO: usize = usize::MAX - 2;
const INIT_ELF: &[u8] = include_bytes!(env!("USER_INIT_PATH"));

static USERS_ALIVE: AtomicUsize = AtomicUsize::new(0);

static DID_SPAWN: AtomicBool = AtomicBool::new(false);

static USER_BASE: AtomicU64 = AtomicU64::new(DEFAULT_USER_BASE);

/// The VA user images are loaded at (see `arch::upaging::pick_user_base`).
#[allow(dead_code)]
pub fn user_base() -> u64 {
    USER_BASE.load(Ordering::SeqCst)
}


/// Enable user mode on the BSP.
pub fn init() {
    crate::arch::user_init();
}

/// Per-CPU user-mode enable.
pub fn ap_init() {
    crate::arch::user_ap_init();
}

/// Load the nested `user/init` ELF at USER_BASE and spawn one process.
pub fn spawn_init() {
    let base = pick_user_base();
    USER_BASE.store(base, Ordering::SeqCst);
    let (aspace, entry, span, off) = load_user_elf(INIT_ELF, true).expect("init ELF");
    let (rsp, argv) = build_argv_stack(aspace, base, off, &[], &[], &[]).expect("init stack");
    task::spawn_user(aspace, entry, rsp, base, span, off, 0, argv);
    USERS_ALIVE.fetch_add(1, Ordering::SeqCst);
    DID_SPAWN.store(true, Ordering::SeqCst);
}

/// Scratch for `load_user_elf` / `reload_user_elf` / `expand_user_elf`.
///
/// Backed by bump-allocator frames (HHDM-contiguous), not kernel `.bss`.
/// Putting `MAX_ELF_PAGES` pages in BSS grew the Limine-loaded image by ~1.5
/// MiB and let the frame bump walk into the kernel physical range on AArch64.
///
/// Allocate only as many contiguous frames as this call needs (grow on demand
/// up to [`MAX_ELF_PAGES`]). Always grabbing the max made UEFI init fail when
/// `MAX_INIT_PAGES` was raised to 3072 for feat_common_core uutils.
const ELF_SCRATCH_BYTES: usize = MAX_ELF_PAGES * PAGE;
// Image, stack, heap and mmap window must fit the per-process span.
const _: () = assert!(
    MAX_ELF_PAGES + USER_STACK_PAGES + HEAP_PAGES + MMAP_AREA_PAGES <= crate::arch::USER_SPAN_PAGES
);

pub fn both_exited() -> bool {
    DID_SPAWN.load(Ordering::SeqCst) && USERS_ALIVE.load(Ordering::SeqCst) == 0
}

pub fn note_exit() {
    USERS_ALIVE.fetch_sub(1, Ordering::SeqCst);
}

pub fn note_fork() {
    USERS_ALIVE.fetch_add(1, Ordering::SeqCst);
}

const S_IFDIR: u32 = 0o040000;
