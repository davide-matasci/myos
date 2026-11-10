//! Byte-level access to another address space through the HHDM, and
//! range checks for user buffers.

use super::*;

pub(super) fn write_user_bytes(aspace: u64, va: usize, src: &[u8]) -> bool {
    each_user_page(aspace, va, src.len(), Access::Write, |p, done, n| unsafe {
        core::ptr::copy_nonoverlapping(src.as_ptr().add(done), p, n);
    })
}

/// Walk `va..va+len` page by page: `f(hhdm pointer, offset into the
/// buffer, bytes in this page)`. An mmap page of the current process not
/// touched yet is paged in for `access` (never under `TASKS`: `fault_in`
/// takes it). False if a page is unmapped, or the range leaves the user
/// window: a user address space shares the kernel's tables, so a kernel
/// address would translate too (a syscall that forgot its range check
/// must not read or write the kernel through a user pointer).
fn each_user_page(aspace: u64, va: usize, len: usize, access: Access, mut f: impl FnMut(*mut u8, usize, usize)) -> bool {
    if va < PAGE || va.checked_add(len).is_none_or(|end| end > USER_WINDOW as usize) {
        return false;
    }
    let mut done = 0;
    while done < len {
        let at = va + done;
        let page = at & !0xfff;
        let off = at & 0xfff;
        let n = (PAGE - off).min(len - done);
        let phys = virt_to_phys(aspace, page as u64).or_else(|| {
            (aspace == task::current_aspace() && fault_in(page, access))
                .then(|| virt_to_phys(aspace, page as u64))
                .flatten()
        });
        let Some(phys) = phys else {
            return false;
        };
        if phys == 0 {
            // V-set/phys-0 leaf (corrupt PTE): never dereference hhdm(0).
            return false;
        }
        // A page mapped without write permission is one the page cache
        // shares (a private mapping's, copied at its first store) or one a
        // fork shares (`LEAF_COW`, copied the same way), or a read-only
        // page: a store from userspace would fault, so the copy takes the
        // same way through `fault_in`, which copies the page or refuses.
        let writable = crate::arch::upaging::leaf_mut(aspace, page as u64)
            .is_some_and(|p| crate::arch::upaging::leaf_writable(unsafe { *p }));
        let phys = if access == Access::Write && !writable {
            if aspace == task::current_aspace() && fault_in(page, Access::Write) {
                match virt_to_phys(aspace, page as u64) {
                    Some(own) if own != 0 => own,
                    _ => return false,
                }
            } else {
                return false;
            }
        } else {
            phys
        };
        f(unsafe { mm::hhdm(phys).add(off) }, done, n);
        done += n;
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

#[allow(dead_code)]
pub fn try_read_user_u8(aspace: u64, va: usize) -> Option<u8> {
    let mut b = [0u8; 1];
    read_user_bytes(aspace, va, &mut b).then_some(b[0])
}

pub(super) fn read_user_bytes(aspace: u64, va: usize, dst: &mut [u8]) -> bool {
    unsafe { copy_from_user_raw(aspace, va, dst.as_mut_ptr(), dst.len()) }
}

/// [`copy_from_user`] into memory that need not be initialized: when it
/// returns true, all `len` bytes at `dst` are written.
///
/// # Safety
/// `dst` must be valid for writes of `len` bytes.
pub unsafe fn copy_from_user_raw(aspace: u64, va: usize, dst: *mut u8, len: usize) -> bool {
    each_user_page(aspace, va, len, Access::Read, |p, done, n| unsafe {
        core::ptr::copy_nonoverlapping(p, dst.add(done), n);
    })
}

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
    task::user_range_ok(ptr, len)
}
