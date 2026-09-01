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
    // TinyCC: write a tiny C file and compile+run it. The needle is printed by
    // the JIT'd `main` (SYS_WRITE via newlib `write` resolved in tcc -run),
    // not by heap itself.
    const HI_C: &[u8] = b"int write(int, const void *, unsigned long);\nint main(void) { write(1, \"tcc ok\\n\", 7); return 0; }\n";
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
    write(b"smoke ok\n");
    exit();
}

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    myos_user::panic_die(info);
}
