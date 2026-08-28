fn main() -> ! {
    // `println!` needs more stack than the 4 KiB user mapping; smoke-test std
    // link/_start with the same marker string the CI needle expects.
    let msg = b"std ok\n";
    unsafe {
        core::arch::asm!(
            "syscall",
            in("rax") 0usize,
            in("rdi") msg.as_ptr() as usize,
            in("rsi") msg.len(),
            lateout("rax") _,
            out("rcx") _,
            out("r11") _,
            options(nostack),
        );
        core::arch::asm!(
            "syscall",
            in("rax") 1usize,
            in("rdi") 0usize,
            options(noreturn),
        );
    }
}
