pub fn print(s: &str) {
    let flags = irq_save();
    irq_off();
    {
        let _hold = SERIAL.lock();
        console::write_str(s);
    }
    irq_restore(flags);
}

pub fn print_bytes(bytes: &[u8]) {
    match core::str::from_utf8(bytes) {
        Ok(s) => print(s),
        Err(_) => {
            let flags = irq_save();
            irq_off();
            {
                let _hold = SERIAL.lock();
                for &b in bytes {
                    console::write_byte(b);
                }
            }
            irq_restore(flags);
        }
    }
}

extern "C" fn trampoline() -> ! {
    let (entry, user_rip, user_rsp, user_argc, user_argv, fork_regs) = {
        let flags = irq_save();
        irq_off();
        let id = CURRENT.load(Ordering::SeqCst);
        let mut tasks = TASKS.lock();
        let t = &mut tasks[id];
        let fr = t.fork_regs.take();
        let out = (
            t.entry,
            t.user_rip,
            t.user_rsp,
            t.user_argc,
            t.user_argv,
            fr,
        );
        drop(tasks);
        irq_restore(flags);
        out
    };
    if let Some(fr) = fork_regs {
        crate::user::enter_fork(fr);
    }
    if user_rip != 0 {
        crate::user::enter(user_rip, user_rsp, user_argc, user_argv);
    }
    irq_on();
    if let Some(f) = entry {
        f();
    }
    die()
}

pub fn die() -> ! {
    irq_off();
    {
        let mut tasks = TASKS.lock();
        let id = CURRENT.load(Ordering::SeqCst);
        if tasks[id].user_rip != 0 {
            user::note_exit();
            for entry in tasks[id].fds {
                fd_drop(entry);
            }
            tasks[id].fds = [FdEntry::Empty; MAX_FDS];
        }
        tasks[id].state = State::Dead;
        tasks[id].entry = None;
    }
    schedule();
    loop {
        irq_on();
        wait();
        irq_off();
        schedule();
    }
}

fn irq_save() -> u64 {
    #[cfg(target_arch = "x86_64")]
    unsafe {
        let r: u64;
        core::arch::asm!("pushfq; pop {r}", r = out(reg) r);
        r
    }
    #[cfg(target_arch = "aarch64")]
    unsafe {
        let r: u64;
        core::arch::asm!(
            "mrs {r}, daif",
            r = out(reg) r,
            options(nomem, nostack, preserves_flags)
        );
        r
    }
    #[cfg(target_arch = "riscv64")]
    unsafe {
        let r: u64;
        core::arch::asm!(
            "csrr {r}, sstatus",
            r = out(reg) r,
            options(nomem, nostack, preserves_flags)
        );
        r
    }
}

fn irq_restore(flags: u64) {
    #[cfg(target_arch = "x86_64")]
    unsafe {
        if flags & (1 << 9) != 0 {
            core::arch::asm!("sti", options(nomem, nostack, preserves_flags));
        } else {
            core::arch::asm!("cli", options(nomem, nostack, preserves_flags));
        }
    }
    #[cfg(target_arch = "aarch64")]
    unsafe {
        core::arch::asm!("msr daif, {r}", r = in(reg) flags, options(nomem, nostack));
    }
    #[cfg(target_arch = "riscv64")]
    unsafe {
        if flags & (1 << 1) != 0 {
            core::arch::asm!("csrs sstatus, {}", in(reg) 1 << 1, options(nomem, nostack));
        } else {
            core::arch::asm!("csrc sstatus, {}", in(reg) 1 << 1, options(nomem, nostack));
        }
    }
}

fn irq_off() {
    #[cfg(target_arch = "x86_64")]
    unsafe {
        core::arch::asm!("cli", options(nomem, nostack, preserves_flags));
    }
    #[cfg(target_arch = "aarch64")]
    unsafe {
        core::arch::asm!("msr daifset, #3", options(nomem, nostack));
    }
    #[cfg(target_arch = "riscv64")]
    unsafe {
        core::arch::asm!("csrc sstatus, {}", in(reg) 1 << 1, options(nomem, nostack));
    }
}

fn irq_on() {
    #[cfg(target_arch = "x86_64")]
    unsafe {
        core::arch::asm!("sti", options(nomem, nostack, preserves_flags));
    }
    #[cfg(target_arch = "aarch64")]
    unsafe {
        core::arch::asm!("msr daifclr, #3", options(nomem, nostack));
    }
    #[cfg(target_arch = "riscv64")]
    unsafe {
        core::arch::asm!("csrs sstatus, {}", in(reg) 1 << 1, options(nomem, nostack));
    }
}

fn wait() {
    #[cfg(target_arch = "x86_64")]
    unsafe {
        core::arch::asm!("hlt", options(nomem, nostack, preserves_flags));
    }
    #[cfg(target_arch = "aarch64")]
    unsafe {
        core::arch::asm!("wfi", options(nomem, nostack, preserves_flags));
    }
    #[cfg(target_arch = "riscv64")]
    unsafe {
        core::arch::asm!("wfi", options(nomem, nostack, preserves_flags));
    }
}
