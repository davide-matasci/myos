//! Linux syscall compatibility layer, as a kernel module.
//!
//! Registers a foreign *personality* with the kernel
//! (`KernelApi::personality_register`): a task gets it when the `linux`
//! launcher asks for it (`SYS_LINUX_NEXT_EXEC`) and execs, or when a task
//! that has it `execve`s. For such a task the kernel hands every syscall to
//! [`dispatch`], which decodes Linux numbers and arguments, calls the native
//! implementation through the module ABI and returns `-errno` on failure,
//! and asks [`signal::deliver`] to build the Linux `rt_sigframe` for a
//! caught signal. Scope: static-PIE and dynamically linked musl binaries on
//! x86_64, aarch64 and riscv64. See docs/linux-compat.md.
//!
//! State lives here, per task slot (the kernel tracks which slots have the
//! personality); the kernel calls the `on_*` hooks at spawn, fork, thread
//! creation and exec.
//!
//! Each arch module (`x86_64`, `aarch64`, `riscv64`) provides the same
//! items: `MACHINE`, `args`, `syscall` (its number table; aarch64 and
//! riscv64 share `generic`), `stat_bytes`, the signal frame (`deliver`,
//! `sigreturn`), the FP/SIMD image for frames (`FP_BYTES`, `fp_save`,
//! `fp_restore`) and the sigreturn trampoline code (`TRAMP_CODE`).

#![no_std]
#![no_main]

extern crate alloc;

mod abi;
mod files;
mod k;
mod lock;
mod net;
mod signal;
mod sys;
mod thread;
mod tty;

#[cfg(any(target_arch = "aarch64", target_arch = "riscv64"))]
mod generic;

#[cfg(target_arch = "x86_64")]
mod x86_64;
#[cfg(target_arch = "x86_64")]
use x86_64 as arch;
#[cfg(target_arch = "aarch64")]
mod aarch64;
#[cfg(target_arch = "aarch64")]
use aarch64 as arch;
#[cfg(target_arch = "riscv64")]
mod riscv64;
#[cfg(target_arch = "riscv64")]
use riscv64 as arch;

use core::alloc::{GlobalAlloc, Layout};

use myos_abi::{ABI_VERSION, KernelApi, PersonalityOps, SignalDelivery, status_ok};

use k::user::SyscallRegs;

/// The kernel heap, through the ABI.
struct KernelHeap;

unsafe impl GlobalAlloc for KernelHeap {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        unsafe { (k::api().alloc)(layout.size(), layout.align()) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { (k::api().dealloc)(ptr, layout.size(), layout.align()) }
    }
}

#[global_allocator]
static HEAP: KernelHeap = KernelHeap;

/// A Linux syscall from a task with the personality (see the module docs).
pub fn dispatch(nr: usize, a0: usize, a1: usize, a2: usize, regs: &mut SyscallRegs) -> usize {
    let args = arch::args(regs, a0, a1, a2);
    arch::syscall(nr, args, regs)
}

unsafe extern "C" fn op_syscall(nr: usize, a0: usize, a1: usize, a2: usize, regs: *mut u64) -> usize {
    let mut regs = SyscallRegs(regs);
    dispatch(nr, a0, a1, a2, &mut regs)
}

/// Hook: a successful exec replaced the image in `slot`.
unsafe extern "C" fn op_on_exec(slot: usize) {
    signal::on_exec(slot);
    let close = files::take_cloexec(slot);
    for fd in (0..64).filter(|fd| close & 1 << fd != 0) {
        sys::close(fd).ok();
    }
}

/// Hook: `child` was forked from `parent` (TASKS held, irqs off).
unsafe extern "C" fn op_on_fork(parent: usize, child: usize) {
    files::on_fork(parent, child);
    signal::on_fork(parent, child);
    thread::on_new_task(child);
}

/// Hook: thread `creator` started thread `slot` in its process. Process-wide
/// state (fd paths, the signal trampoline) is kept per process.
unsafe extern "C" fn op_on_thread(_creator: usize, slot: usize) {
    thread::on_new_task(slot);
}

/// Hook: `slot` was (re)used for a freshly spawned task.
unsafe extern "C" fn op_on_spawn(slot: usize) {
    files::on_spawn(slot);
    signal::on_exec(slot);
    thread::on_new_task(slot);
}

unsafe extern "C" fn op_deliver(regs: *mut u64, d: *const SignalDelivery, out: *mut usize) -> i32 {
    if regs.is_null() || d.is_null() || out.is_null() {
        return -1;
    }
    let mut regs = SyscallRegs(regs);
    match signal::deliver(&mut regs, unsafe { &*d }) {
        Some(v) => {
            unsafe { *out = v };
            0
        }
        None => -1,
    }
}

static OPS: PersonalityOps = PersonalityOps {
    // musl (rv64gc) uses the D extension; native riscv64 programs are
    // soft-float, so only Linux tasks run with `sstatus.FS` on.
    flags: if cfg!(target_arch = "riscv64") { myos_abi::PERSONALITY_FPU_ON } else { 0 },
    syscall: op_syscall,
    on_exec: op_on_exec,
    on_fork: op_on_fork,
    on_thread: op_on_thread,
    on_spawn: op_on_spawn,
    deliver: op_deliver,
};

#[inline(never)]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn module_init(api_ptr: *const KernelApi) -> i32 {
    if api_ptr.is_null() {
        return -1;
    }
    let api: &'static KernelApi = unsafe { &*api_ptr };
    if api.abi_version != ABI_VERSION {
        return -2;
    }
    k::set_api(api);
    let rc = unsafe { (api.personality_register)(&OPS) };
    if rc == 0 {
        status_ok(api, "linux");
    }
    rc
}

#[inline(never)]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn module_exit() {}

// So `cargo build --bin linux` links. The kernel never jumps here.
#[unsafe(no_mangle)]
pub extern "C" fn _start() -> ! {
    loop {}
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    loop {}
}
