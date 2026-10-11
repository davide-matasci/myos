//! Log CPU exceptions to serial before halting (used by arch interrupt handlers).

extern crate alloc;

use alloc::format;
use alloc::string::String;

use crate::arch;
use crate::console;
use crate::task;

pub fn fatal_line(line: &str) -> ! {
    console::status_fail(&format!("exception: {line}"));
    console::flush();
    arch::exit_qemu(arch::QEMU_FAILURE);
    arch::halt();
}

/// Terminate the current user task after a synchronous fault in EL0/U-mode.
///
/// Prefer this over [`fatal_line`] for translation/permission faults at FAR=0
/// and friends: a userspace null deref must not halt the whole machine (that
/// turned `cat | cat` + ^C into a CI `[ FAIL ] exception` on aarch64). Exit
/// status matches the shell convention for SIGSEGV (`128 + 11`).
/// Warn-only variant: print and return. Used before `user_fault_kill` when
/// the detail is split across two lines and the harness may kill QEMU on the
/// first WARN before the second line reaches serial.
#[allow(dead_code)]
pub fn user_fault_warn(kind: &str, detail: &str) {
    console::status_warn(&format!("user fault: {kind} {detail}"));
    console::flush();
}

/// `pc` is where the task faulted: when it lies in a file mapping (a shared
/// library, a program mapped like one) the line names the file and the
/// address in it, which `addr2line` or a disassembler take as they are
/// (`docs/linux-compat.md`, Debugging).
pub fn user_fault_kill(kind: &str, pc: u64, detail: &str) -> ! {
    // Avoid the substring `exception:` so the boot test host, which ends the run on it,
    // treat any `exception:` as a hard fail stay quiet when a *child* faults.
    console::status_warn(&format!("user fault: {kind}{} {detail}", location(pc)));
    console::flush();
    crate::user::set_syscall_frame(core::ptr::null_mut());
    task::user_exit(128u8.wrapping_add(11));
}

/// ` in FILE+0xADDR` for an address in a file mapping of the current
/// process: `ADDR` is the ELF virtual address in the file (what `nm`,
/// `objdump` and `addr2line` take), from its program headers; for a file
/// that is no ELF, ` in FILE at file offset 0xOFF`. ` in anonymous memory`
/// in another mapping, nothing elsewhere (the image, the stack, no mapping).
fn location(pc: u64) -> String {
    let Some((_, file, _)) = task::mmap_backing(pc as usize) else {
        return String::new();
    };
    let Some((node, page_off)) = file else {
        return String::from(" in anonymous memory");
    };
    let off = page_off as u64 + pc % crate::user::PAGE as u64;
    let path = crate::fs::vfs::vnode_path(&node);
    // The program headers are in the file's first page in practice.
    let mut head = alloc::vec![0u8; crate::user::PAGE];
    let n = crate::fs::read(&node, 0, &mut head);
    let mut addr = None;
    let _ = crate::modules::elf::for_each_load_segment(&head[..n], |seg| {
        if (seg.offset..seg.offset + seg.filesz).contains(&off) {
            addr = Some(seg.vaddr + (off - seg.offset));
        }
    });
    match addr {
        Some(addr) => format!(" in {path}+{addr:#x}"),
        None => format!(" in {path} at file offset {off:#x}"),
    }
}

pub fn task_ctx() -> String {
    let id = task::current_id();
    // A dead canary says the task ran off its kernel stack before the fault.
    let stack = match task::current_stack_intact() {
        Some(false) => " kstack overflowed",
        _ => "",
    };
    match task::current_user_pc_sp() {
        Some((rip, rsp)) => format!(" task={id} user rip={rip:#x} rsp={rsp:#x}{stack}"),
        None => format!(" task={id} kernel{stack}"),
    }
}
