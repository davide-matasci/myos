//! Byte-level access to another address space through the HHDM, and
//! range checks for user buffers.

use super::*;

pub(super) fn write_user_byte(aspace: u64, va: usize, byte: u8) -> bool {
    let page = va & !0xfff;
    let off = va & 0xfff;
    let Some(phys) = virt_to_phys(aspace, page as u64) else {
        return false;
    };
    unsafe {
        *mm::hhdm(phys).add(off) = byte;
    }
    true
}

pub(super) fn write_user_bytes(aspace: u64, va: usize, src: &[u8]) -> bool {
    for (i, &b) in src.iter().enumerate() {
        if !write_user_byte(aspace, va + i, b) {
            return false;
        }
    }
    true
}

/// Copy bytes from the current user address space into a kernel buffer.
pub fn copy_from_user(aspace: u64, va: usize, dst: &mut [u8]) -> bool {
    read_user_bytes(aspace, va, dst)
}

/// Copy bytes from a kernel buffer into the current user address space.
pub fn copy_to_user(aspace: u64, va: usize, src: &[u8]) -> bool {
    write_user_bytes(aspace, va, src)
}

pub(super) fn write_user_usize(aspace: u64, va: usize, val: usize) -> bool {
    write_user_bytes(aspace, va, &val.to_le_bytes())
}

#[cfg(not(target_arch = "aarch64"))]
pub fn try_read_user_u8(aspace: u64, va: usize) -> Option<u8> {
    read_user_byte(aspace, va)
}

fn read_user_byte(aspace: u64, va: usize) -> Option<u8> {
    let page = va & !0xfff;
    let off = va & 0xfff;
    let phys = virt_to_phys(aspace, page as u64)?;
    if phys == 0 {
        // V-set/phys-0 leaf (corrupt PTE): never dereference hhdm(0).
        return None;
    }
    Some(unsafe { *mm::hhdm(phys).add(off) })
}

pub(super) fn read_user_bytes(aspace: u64, va: usize, dst: &mut [u8]) -> bool {
    for (i, b) in dst.iter_mut().enumerate() {
        *b = match read_user_byte(aspace, va + i) {
            Some(v) => v,
            None => return false,
        };
    }
    true
}

#[cfg(any(target_arch = "aarch64", target_arch = "riscv64"))]
pub(super) fn read_user_usize(aspace: u64, va: usize) -> Option<usize> {
    let mut buf = [0u8; core::mem::size_of::<usize>()];
    if !read_user_bytes(aspace, va, &mut buf) {
        return None;
    }
    Some(usize::from_le_bytes(buf))
}

/// True when `ptr..ptr+len` lies in the current task's user code, stack, heap, or mmap.
pub fn buffer_ok(ptr: usize, len: usize) -> bool {
    user_range_ok(ptr, len)
}

pub(super) fn user_range_ok(ptr: usize, len: usize) -> bool {
    let (base, image_span, stack_off) = task::current_user_map();
    let base = base as usize;
    let end = match ptr.checked_add(len) {
        Some(e) => e,
        None => return false,
    };
    let in_code = ptr >= base && end <= base + image_span;
    let stack_base = base + stack_off as usize;
    let in_stack = ptr >= stack_base && end <= stack_base + USER_STACK_PAGES * PAGE;
    let heap_base = heap_base_va(base as u64, stack_off) as usize;
    let brk = task::current_brk() as usize;
    let in_heap = brk > heap_base && ptr >= heap_base && end <= brk;
    if in_code || in_stack || in_heap {
        return true;
    }
    // mmap_contains takes TASKS; skip unless ptr is outside code/stack/heap.
    task::mmap_contains(ptr, len)
}
