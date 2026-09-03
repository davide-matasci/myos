#![no_std]
#![no_main]

//! CI-only heavy smoke: std / C / sbase / uutils / ripgrep / tcc / bigalloc.
//! Always-on boot uses slim `/ok` instead; `wait_ci` types `heap` at `$`.

use myos_user::{
    close, exec, exit, exit_code, fork, open_flags, wait_status, write, write_fd, O_CREAT,
    O_TRUNC, O_WRONLY,
};

#[cfg(target_arch = "x86_64")]
#[unsafe(no_mangle)]
pub extern "C" fn _start() -> ! {
    main()
}

#[cfg(any(target_arch = "aarch64", target_arch = "riscv64"))]
#[unsafe(no_mangle)]
pub extern "C" fn _start(_argc: usize, _argv: *const usize) -> ! {
    main()
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

fn run_prog_exit(path: &[u8], args: &[&[u8]], expect: u8, ok_msg: &[u8]) {
    match fork() {
        None => write(b"fork fail\n"),
        Some(0) => {
            exec(path, args);
            exit_code(127);
        }
        Some(_) => {
            if let Some((_, status)) = wait_status() {
                if status == expect {
                    write(ok_msg);
                }
            }
        }
    }
}

fn main() -> ! {
    write(b"smoke start\n");
    run_prog_exit(b"/bigalloc", &[], 0, b"bigalloc ok\n");
    run_prog_exit(b"/c/echo", &[], 0, b"uutils echo ok\n");
    run_prog_exit(b"/c/true", &[], 0, b"uutils true ok\n");
    run_prog_exit(b"/c/false", &[], 1, b"uutils false ok\n");
    // Write a needle under /tmp and search with /c/rg (full ripgrep + PCRE2).
    // -j1 / --no-mmap / --no-config: myos is single-threaded; rg mmap is optional.
    if let Some(fd) = open_flags(b"/tmp/rg-needle.txt", O_WRONLY | O_CREAT | O_TRUNC) {
        let _ = write_fd(fd, b"hello ripgrep needle world\n");
        close(fd);
        run_prog_exit(
            b"/c/rg",
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
            b"ripgrep ok\n",
        );
    } else {
        write(b"ripgrep skip (tmp create fail)\n");
    }
    run_prog(b"/stdhello", &[]);
    run_prog(b"/stdcat", &[]);
    run_prog(b"/stdecho", &[]);
    run_prog(b"/chello", &[]);
    run_prog(b"/s/true", &[]);
    run_prog(b"/s/echo", &[]);
    run_prog(b"/s/ls", &[]);
    run_prog(b"/s/echo", &[b"echo", b"sbase argv ok"]);
    run_prog(b"/s/ls", &[b"ls", b"/s"]);
    run_prog(b"/s/pwd", &[]);
    // TinyCC JIT: -nostdlib skips libgloss, so hi.c emits SYS_WRITE=0 itself.
    // Needle is printed by the JIT'd main, not by heap.
    const HI_C: &[u8] = br#"
__attribute__((used))
long write(int fd, const void *buf, unsigned long n);
#ifdef __x86_64__
__asm__(".text\n.globl write\nwrite:\n mov $0, %rax\n syscall\n ret\n");
#elif defined(__aarch64__)
__asm__(".text\n.globl write\nwrite:\n mov x8, 0\n .int 0xd4000001\n ret\n");
#elif defined(__riscv)
__asm__(".text\n.globl write\nwrite:\n li a7, 0\n ecall\n ret\n");
#endif
int main(void) { write(1, "tcc ok\n", 7); return 0; }
"#;
    if let Some(fd) = open_flags(b"/tmp/hi.c", O_WRONLY | O_CREAT | O_TRUNC) {
        let _ = write_fd(fd, HI_C);
        close(fd);
        run_prog(
            b"/t/tcc",
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
    puts("tcc std ok");
    return 0;
}
"#;
    if let Some(fd) = open_flags(b"/tmp/tcc-std.c", O_WRONLY | O_CREAT | O_TRUNC) {
        let _ = write_fd(fd, STD_C);
        close(fd);
        run_prog(
            b"/t/tcc",
            &[b"tcc", b"-o", b"/tmp/tcc-hi", b"/tmp/tcc-std.c"],
        );
        run_prog(b"/tmp/tcc-hi", &[b"tcc-hi"]);
    } else {
        write(b"tcc std skip (tmp create fail)\n");
    }
    // ICMP echo via /net/icmp (netd); needle is printed by /ping, not heap.
    // 10.0.2.2 is QEMU slirp gateway; 1.1.1.1 often fails through -netdev user.
    run_prog(b"/ping", &[b"ping", b"10.0.2.2"]);
    write(b"smoke ok\n");
    exit();
}

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    myos_user::panic_die(info);
}
