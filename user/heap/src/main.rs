#![no_std]
#![deny(unsafe_op_in_unsafe_fn)]
#![no_main]

//! CI-only heavy smoke: std / C / sbase / uutils (full mode) / ripgrep / tcc / bigalloc.
//! Always-on boot uses slim `/ok` instead; the boot tests run `heap` (user/tests/shell.sh).

use core::cell::UnsafeCell;

use myos_user::{status_ok, 
    close, exec, exit, exit_code, fork, mkdir, open_flags, wait_status, write, write_fd, O_CREAT,
    O_TRUNC, O_WRONLY,
};

myos_user::x86_start!(main);

#[cfg(any(target_arch = "aarch64", target_arch = "riscv64"))]
#[unsafe(no_mangle)]
pub extern "C" fn _start(argc: usize, argv: *const usize) -> ! {
    unsafe { myos_user::args::init_from_regs(argc, argv) };
    main()
}

/// The mini test list passes `heap mini`: the mini boots keep their fast turnaround,
/// so the heavy git porcelain stage only runs in the full boot jobs.
fn mini_mode() -> bool {
    (0..myos_user::argc()).any(|i| myos_user::arg(i) == Some(b"mini" as &[u8]))
}

fn run_prog(path: &[u8], args: &[&[u8]]) {
    match fork() {
        None => write(b"fork fail\n"),
        Some(0) => {
            exec(path, args);
            exit_code(127);
        }
        Some(_) => {
            let _ = wait_status();
        }
    }
}

fn run_prog_exit(path: &[u8], args: &[&[u8]], expect: u8, ok_msg: &[u8]) -> bool {
    match fork() {
        None => {
            write(b"fork fail\n");
            false
        }
        Some(0) => {
            exec(path, args);
            exit_code(127);
        }
        Some(_) => {
            if let Some((_, status)) = wait_status() {
                if status == expect {
                    write(ok_msg);
                    return true;
                }
                write(b"prog bad status\n");
            } else {
                write(b"prog wait fail\n");
            }
            false
        }
    }
}

fn main() -> ! {
    write(b"smoke start\n");
    let _ = run_prog_exit(b"/bin/std/bigalloc", &[], 0, b"[ OK ] bigalloc\n");
    // uutils coreutils is a package: the full mode installs it before the
    // tests, the mini list (`heap mini`) runs without it.
    let uutils = !mini_mode();
    if uutils {
        let _ = run_prog_exit(b"/bin/coreutils/echo", &[], 0, b"[ OK ] uutils echo\n");
        let _ = run_prog_exit(b"/bin/coreutils/true", &[], 0, b"[ OK ] uutils true\n");
        let _ = run_prog_exit(b"/bin/coreutils/false", &[], 1, b"[ OK ] uutils false\n");
    } else {
        write(b"uutils skip (boot-mini)\n");
    }
    // Recursive sbase find: needs multi-DIR libgloss (nested opendir while walking).
    if mkdir(b"/tmp/findnest")
        && mkdir(b"/tmp/findnest/a")
        && mkdir(b"/tmp/findnest/a/b")
    {
        if let Some(fd) = open_flags(b"/tmp/findnest/a/b/c", O_WRONLY | O_CREAT | O_TRUNC) {
            let _ = write_fd(fd, b"nest\n");
            close(fd);
            let _ = run_prog_exit(
                b"/bin/sbase/find",
                &[b"find", b"/tmp/findnest"],
                0,
                b"[ OK ] find\n",
            );
        }
    } else {
        write(b"find skip (mkdir fail)\n");
    }
    // Newly-ported uutils that need std::fs::read_dir / open.
    if uutils {
        if !run_prog_exit(
            b"/bin/coreutils/cat",
            &[b"cat", b"/tmp/findnest/a/b/c"],
            0,
            b"[ OK ] uutils cat\n",
        ) {
            write(b"uutils cat findnest failed; retry /msg\n");
            let _ = run_prog_exit(b"/bin/coreutils/cat", &[b"cat", b"/msg"], 0, b"[ OK ] uutils cat\n");
        }
        let _ = run_prog_exit(b"/bin/coreutils/ls", &[b"ls", b"/tmp/findnest"], 0, b"[ OK ] uutils ls\n");
    }
    // Write a needle under /tmp and search with /c/rg (full ripgrep + PCRE2).
    // -j1 / --no-mmap / --no-config: rg mmap is optional; the threaded search
    // is ports/ripgrep/test.sh's.
    if let Some(fd) = open_flags(b"/tmp/rg-needle.txt", O_WRONLY | O_CREAT | O_TRUNC) {
        let _ = write_fd(fd, b"hello ripgrep needle world\n");
        close(fd);
        // Hard-fail: a silent "prog bad status" after sepc=0 IPF let heap continue
        // and only the host needle missed the regression. Keep running rg — do not
        // skip on fault — but stop the suite if the child dies.
        if !run_prog_exit(
            b"/bin/coreutils/rg",
            &[
                b"rg",
                b"-j",
                b"1",
                b"--color=never",
                b"--no-config",
                b"--no-mmap",
                b"needle",
                b"/tmp/rg-needle.txt",
            ],
            0,
            b"[ OK ] ripgrep\n",
        ) {
            write(b"ripgrep failed (expect sepc=0 IPF if kernel exec/tp broken)\n");
            exit_code(1);
        }
    } else {
        write(b"ripgrep skip (tmp create fail)\n");
    }
    run_prog(b"/bin/std/hello", &[]);
    run_prog(b"/bin/std/cat", &[]);
    run_prog(b"/bin/std/echo", &[]);
    run_prog(b"/bin/etc/hello", &[]);
    run_prog(b"/bin/sbase/true", &[]);
    // sbase's option parser reads argv[0] (unpatched upstream, like most C
    // programs); the no-argv exec is covered by the programs above. The
    // markers come from the exit status: sbase is not patched to print them.
    let _ = run_prog_exit(b"/bin/sbase/echo", &[b"echo"], 0, b"[ OK ] sbase\n");
    let _ = run_prog_exit(b"/bin/sbase/ls", &[b"ls"], 0, b"[ OK ] sls\n");
    run_prog(b"/bin/sbase/echo", &[b"echo", b"[ OK ] sbase argv"]);
    run_prog(b"/bin/sbase/ls", &[b"ls", b"/bin/sbase"]);
    run_prog(b"/bin/sbase/pwd", &[b"pwd"]);
    // TinyCC JIT: -nostdlib skips libgloss, so hi.c makes the write itself:
    // `pwrite` (82) with offset and flags 0, at the file position.
    // Needle is printed by the JIT'd main, not by heap.
    const HI_C: &[u8] = br#"
__attribute__((used))
long write(int fd, const void *buf, unsigned long n);
#ifdef __x86_64__
__asm__(".text\n.globl write\nwrite:\n mov $82, %rax\n xor %r10, %r10\n xor %r8, %r8\n syscall\n ret\n");
#elif defined(__aarch64__)
__asm__(".text\n.globl write\nwrite:\n mov x8, 82\n mov x3, 0\n mov x4, 0\n .int 0xd4000001\n ret\n");
#elif defined(__riscv)
__asm__(".text\n.globl write\nwrite:\n li a7, 82\n li a3, 0\n li a4, 0\n ecall\n ret\n");
#endif
int main(void) { write(1, "[ OK ] tcc\n", 11); return 0; }
"#;
    if let Some(fd) = open_flags(b"/tmp/hi.c", O_WRONLY | O_CREAT | O_TRUNC) {
        let _ = write_fd(fd, HI_C);
        close(fd);
        run_prog(
            b"/bin/tcc/tcc",
            &[b"tcc", b"-nostdlib", b"-run", b"/tmp/hi.c"],
        );
    } else {
        write(b"tcc skip (tmp create fail)\n");
    }
    // Hosted tcc: default crt + libc + libgloss, PIE (ET_DYN) ELF, then exec.
    // Needle is printed by the compiled program, not by heap.
    const STD_C: &[u8] = br#"
#include <stdio.h>
int main(void) {
    puts("[ OK ] tcc std");
    return 0;
}
"#;
    if let Some(fd) = open_flags(b"/tmp/tcc-std.c", O_WRONLY | O_CREAT | O_TRUNC) {
        let _ = write_fd(fd, STD_C);
        close(fd);
        run_prog(
            b"/bin/tcc/tcc",
            &[b"tcc", b"-o", b"/tmp/tcc-hi", b"/tmp/tcc-std.c"],
        );
        run_prog(b"/tmp/tcc-hi", &[b"tcc-hi"]);
    } else {
        write(b"tcc std skip (tmp create fail)\n");
    }
    // Phase-1 git porcelain (offline): init/add/commit/log on tmpfs.
    // Absolute /bin/custom/git — PATH is fine at login, but heap execs by path.
    // Skipped in the mini list (`heap mini`): git testing belongs to the full boot
    // jobs, which have time for the Phase-1 exec pages.
    if mini_mode() {
        write(b"git skip (boot-mini)\n");
    } else if mkdir(b"/tmp/gittest") {
        let _ = run_prog_exit(
            b"/bin/custom/git",
            &[b"git", b"-C", b"/tmp/gittest", b"init"],
            0,
            b"[ OK ] git\n",
        );
        run_prog(
            b"/bin/custom/git",
            &[
                b"git",
                b"-C",
                b"/tmp/gittest",
                b"config",
                b"user.email",
                b"ci@myos",
            ],
        );
        run_prog(
            b"/bin/custom/git",
            &[
                b"git",
                b"-C",
                b"/tmp/gittest",
                b"config",
                b"user.name",
                b"myos-ci",
            ],
        );
        if let Some(fd) = open_flags(b"/tmp/gittest/f.txt", O_WRONLY | O_CREAT | O_TRUNC) {
            let _ = write_fd(fd, b"hello git\n");
            close(fd);
            run_prog(
                b"/bin/custom/git",
                &[b"git", b"-C", b"/tmp/gittest", b"add", b"f.txt"],
            );
            let _ = run_prog_exit(
                b"/bin/custom/git",
                &[
                    b"git",
                    b"-C",
                    b"/tmp/gittest",
                    b"commit",
                    b"-m",
                    b"t",
                ],
                0,
                b"[ OK ] git commit\n",
            );
            run_prog(
                b"/bin/custom/git",
                &[b"git", b"-C", b"/tmp/gittest", b"log", b"--oneline"],
            );
            run_prog(
                b"/bin/custom/git",
                &[b"git", b"-C", b"/tmp/gittest", b"status"],
            );
        } else {
            write(b"git skip (create f.txt fail)\n");
        }
    } else {
        write(b"git skip (mkdir fail)\n");
    }
    // ICMP echo via /net/icmp (netd); needle is printed by /ping, not heap.
    // 10.0.2.2 is QEMU slirp gateway; 1.1.1.1 often fails through -netdev user.
    run_prog(b"/bin/custom/ping", &[b"ping", b"10.0.2.2"]);
    // Userspace BSD sockets over /net/tcp (no socket syscall).
    run_prog(b"/bin/etc/socket_smoke", &[b"socket_smoke"]);
    fpu_smoke();
    thread_smoke();
    status_ok("smoke");
    exit();
}

/// Stacks for the threads of [`thread_smoke`].
#[repr(C, align(16))]
struct Stack(UnsafeCell<[u8; 16 * 1024]>);
// SAFETY: nothing reads or writes a stack as data: each is only the stack
// of the one thread `thread_smoke` starts on it.
unsafe impl Sync for Stack {}
static STACKS: [Stack; 5] = [const { Stack(UnsafeCell::new([0; 16 * 1024])) }; 5];

fn stack_top(i: usize) -> usize {
    STACKS[i].0.get() as usize + core::mem::size_of::<Stack>()
}

/// How many threads `/proc/<pid>/task` lists for process `pid`.
fn proc_threads(pid: usize) -> usize {
    let mut path = [0u8; 32];
    let mut n = 0;
    for &b in b"/proc/" {
        path[n] = b;
        n += 1;
    }
    let mut digits = [0u8; 20];
    let mut d = 0;
    let mut v = pid;
    loop {
        digits[d] = b'0' + (v % 10) as u8;
        d += 1;
        v /= 10;
        if v == 0 {
            break;
        }
    }
    while d > 0 {
        d -= 1;
        path[n] = digits[d];
        n += 1;
    }
    for &b in b"/task" {
        path[n] = b;
        n += 1;
    }
    let mut buf = [0u8; 512];
    let len = myos_user::listdir(&path[..n], &mut buf);
    if len == usize::MAX {
        return 0;
    }
    buf[..len].iter().filter(|&&b| b == b'\n').count()
}

/// Threads share the process's memory: workers bump a shared counter and
/// report through wait/wake on an address. Then a forked child, whose three
/// threads `/proc/<pid>/task` must list, exits while one of its threads
/// sleeps on an address and another spins in user mode (never making a
/// syscall): both must end with it.
fn thread_smoke() {
    use core::sync::atomic::{AtomicU32, Ordering::SeqCst};
    use myos_user::thread;
    const WORKERS: usize = 3;
    const ROUNDS: u32 = 1000;
    static COUNT: AtomicU32 = AtomicU32::new(0);
    static DONE: AtomicU32 = AtomicU32::new(0);
    static NEVER: AtomicU32 = AtomicU32::new(0);
    extern "C" fn worker(_: usize) -> ! {
        for _ in 0..ROUNDS {
            COUNT.fetch_add(1, SeqCst);
            core::hint::spin_loop();
        }
        DONE.fetch_add(1, SeqCst);
        thread::wake(&DONE, 1);
        thread::exit(0);
    }
    extern "C" fn sleeper(_: usize) -> ! {
        loop {
            thread::wait(&NEVER, 0, 0);
        }
    }
    extern "C" fn spinner(_: usize) -> ! {
        loop {
            core::hint::spin_loop();
        }
    }
    let pid = myos_user::getpid();
    let mut ok = thread::gettid() == pid;
    for i in 0..WORKERS {
        ok &= thread::spawn(worker, stack_top(i), i, 0).is_some_and(|tid| tid != pid);
    }
    while ok {
        let done = DONE.load(SeqCst);
        if done as usize == WORKERS {
            break;
        }
        ok &= thread::wait(&DONE, done, 0) != thread::Wait::Fault;
    }
    ok &= COUNT.load(SeqCst) == WORKERS as u32 * ROUNDS;
    match fork() {
        Some(0) => {
            let _ = thread::spawn(sleeper, stack_top(WORKERS), 0, 0);
            let _ = thread::spawn(spinner, stack_top(WORKERS + 1), 0, 0);
            myos_user::sleep_ns(50_000_000, false);
            exit_code(if proc_threads(myos_user::getpid()) == 3 { 7 } else { 8 });
        }
        Some(_) => ok &= matches!(wait_status(), Some((_, 7))),
        None => ok = false,
    }
    if ok {
        write(b"[ OK ] threads\n");
    } else {
        write(b"threads FAIL\n");
    }
}

/// Children keep an FP value live in a register across a long loop while
/// siblings do the same with other values: if the kernel did not switch the
/// FP/SIMD registers with the task, preemption would mix them up.
fn fpu_smoke() {
    #[cfg(any(target_arch = "x86_64", target_arch = "aarch64"))]
    {
        const CHILDREN: u64 = 4;
        const ITERS: u64 = 5_000_000;
        for k in 0..CHILDREN {
            match fork() {
                None => write(b"fork fail\n"),
                Some(0) => {
                    let base = 1000.0 * (k + 1) as f64;
                    let ok = fp_spin(base.to_bits(), ITERS) == (base + ITERS as f64).to_bits();
                    exit_code(if ok { 0 } else { 1 });
                }
                Some(_) => {}
            }
        }
        let mut bad = 0;
        for _ in 0..CHILDREN {
            match wait_status() {
                Some((_, 0)) => {}
                _ => bad += 1,
            }
        }
        if bad == 0 {
            write(b"[ OK ] fpu\n");
        } else {
            write(b"fpu FAIL: FP registers changed across preemption\n");
        }
    }
    // Native riscv64 programs are soft-float: no FP registers to switch.
    #[cfg(target_arch = "riscv64")]
    write(b"[ OK ] fpu (soft-float)\n");
}

/// Add 1.0 to `bits` (an f64) `n` times, keeping it in an FP register.
#[cfg(target_arch = "x86_64")]
fn fp_spin(bits: u64, n: u64) -> u64 {
    let out: u64;
    unsafe {
        core::arch::asm!(
            "movq xmm0, {v}",
            "mov {t}, 0x3ff0000000000000",
            "movq xmm1, {t}",
            "2:",
            "addsd xmm0, xmm1",
            "dec {n}",
            "jnz 2b",
            "movq {v}, xmm0",
            v = inout(reg) bits => out,
            n = inout(reg) n => _,
            t = out(reg) _,
            options(nostack, nomem),
        );
    }
    out
}

#[cfg(target_arch = "aarch64")]
fn fp_spin(bits: u64, n: u64) -> u64 {
    let out: u64;
    unsafe {
        core::arch::asm!(
            ".arch_extension fp",
            "fmov d0, {v}",
            "fmov d1, #1.0",
            "2:",
            "fadd d0, d0, d1",
            "subs {n}, {n}, #1",
            "b.ne 2b",
            "fmov {v}, d0",
            v = inout(reg) bits => out,
            n = inout(reg) n => _,
            options(nostack, nomem),
        );
    }
    out
}

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    myos_user::panic_die(info);
}
