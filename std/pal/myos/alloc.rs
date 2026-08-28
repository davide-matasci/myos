//! `GlobalAlloc` backed by myos syscall 9 (`brk`).

use crate::alloc::{GlobalAlloc, Layout};
use crate::sync::atomic::{AtomicUsize, Ordering};

const SYS_BRK: usize = 9;
const PAGE: usize = 4096;

static BRK_END: AtomicUsize = AtomicUsize::new(0);
static BRK_PTR: AtomicUsize = AtomicUsize::new(0);

struct MyosAlloc;

pub fn init() {
    let end = brk(0);
    BRK_END.store(end, Ordering::SeqCst);
    BRK_PTR.store(end, Ordering::SeqCst);
}

unsafe impl GlobalAlloc for MyosAlloc {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let align = layout.align().max(1);
        let size = layout.size().max(1);
        loop {
            let ptr = BRK_PTR.load(Ordering::SeqCst);
            let aligned = (ptr + align - 1) & !(align - 1);
            let Some(new_end) = aligned.checked_add(size) else {
                return core::ptr::null_mut();
            };
            let cur_end = BRK_END.load(Ordering::SeqCst);
            if new_end <= cur_end {
                BRK_PTR.store(new_end, Ordering::SeqCst);
                return aligned as *mut u8;
            }
            let new_brk = (new_end + PAGE - 1) & !(PAGE - 1);
            let got = brk(new_brk);
            if got < new_brk {
                return core::ptr::null_mut();
            }
            BRK_END.store(got, Ordering::SeqCst);
        }
    }

    unsafe fn dealloc(&self, _ptr: *mut u8, _layout: Layout) {}
}

#[global_allocator]
static ALLOCATOR: MyosAlloc = MyosAlloc;

fn brk(addr: usize) -> usize {
    let ret: usize;
    unsafe {
        core::arch::asm!(
            "syscall",
            in("rax") SYS_BRK,
            in("rdi") addr,
            lateout("rax") ret,
            out("rcx") _,
            out("r11") _,
            options(nostack),
        );
    }
    ret
}
