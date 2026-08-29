#![no_std]
#![no_main]

use myos_user::{close, exec, exit, fork, open, read_line, wait, write};

const PROMPT: &[u8] = b"$ ";
const MAX_LINE: usize = 128;
const MAX_ARGS: usize = 8;
const ARG_LEN: usize = 32;

#[cfg(target_arch = "x86_64")]
#[unsafe(no_mangle)]
pub extern "C" fn _start() -> ! {
    shell()
}

#[cfg(any(target_arch = "aarch64", target_arch = "riscv64"))]
#[unsafe(no_mangle)]
pub extern "C" fn _start(argc: usize, argv: *const usize) -> ! {
    unsafe { myos_user::args::init_from_regs(argc, argv) };
    shell()
}

fn shell() -> ! {
    write(b"sh ok\n");
    smoke_fork_ping();
    smoke_fork(b"ok", &[]);
    let mut line = [0u8; MAX_LINE];
    loop {
        write(PROMPT);
        let n = read_line(&mut line);
        if n == 0 {
            continue;
        }
        let mut len = n;
        while len > 0 && (line[len - 1] == b'\n' || line[len - 1] == b'\r') {
            len -= 1;
        }
        if len == 0 {
            continue;
        }
        let mut parts: [&[u8]; MAX_ARGS] = [&[]; MAX_ARGS];
        let argc = split_args(&line[..len], &mut parts);
        if argc == 0 {
            continue;
        }
        if parts[0] == b"exit" {
            exit();
        }
        run_path(parts[0], &parts[..argc]);
    }
}

/// Minimal fork+wait smoke (no exec in the child).
fn smoke_fork_ping() {
    match fork() {
        Some(0) => exit(),
        Some(_) => {
            let _ = wait();
            write(b"fork ok\n");
        }
        None => write(b"fork failed\n"),
    }
}

/// Boot smoke via fork+exec+wait (CI covers fork+exec).
fn smoke_fork(name: &[u8], parts: &[&[u8]]) {
    let mut path_buf = [0u8; 32];
    let path = command_path(name, &mut path_buf);
    let mut arg_bufs = [[0u8; ARG_LEN]; MAX_ARGS];
    let mut arg_slices: [&[u8]; MAX_ARGS] = [&[]; MAX_ARGS];
    for (i, p) in parts.iter().enumerate() {
        let n = p.len().min(ARG_LEN);
        arg_bufs[i][..n].copy_from_slice(&p[..n]);
    }
    for i in 0..parts.len() {
        let n = parts[i].len().min(ARG_LEN);
        arg_slices[i] = &arg_bufs[i][..n];
    }
    match fork() {
        Some(0) => {
            exec(path, &arg_slices[..parts.len()]);
            cmd_not_found(path, name);
            exit();
        }
        Some(_) => {
            let _ = wait();
            write(b"fork exec ok\n");
        }
        None => write(b"fork failed\n"),
    }
}

fn write_hex_byte(b: u8) {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    write(&[HEX[(b >> 4) as usize], HEX[(b & 0xF) as usize]]);
}

fn write_bytes_escaped(bytes: &[u8]) {
    for &b in bytes {
        if (0x20..0x7E).contains(&b) && b != b'\\' {
            write(&[b]);
        } else {
            write(b"\\x");
            write_hex_byte(b);
        }
    }
}

fn cmd_not_found(path: &[u8], cmd: &[u8]) {
    write(b"sh: command not found: ");
    write_bytes_escaped(path);
    write(b" (received: ");
    write_bytes_escaped(cmd);
    write(b")\n");
}

fn run_path(name: &[u8], parts: &[&[u8]]) {
    let mut cmd = [0u8; ARG_LEN];
    let cmd_len = name.len().min(ARG_LEN);
    cmd[..cmd_len].copy_from_slice(&name[..cmd_len]);
    let name = &cmd[..cmd_len];

    let mut path_buf = [0u8; 32];
    let path = command_path(name, &mut path_buf);
    let Some(fd) = open(path) else {
        cmd_not_found(path, name);
        return;
    };
    close(fd);
    let mut arg_bufs = [[0u8; ARG_LEN]; MAX_ARGS];
    let mut arg_slices: [&[u8]; MAX_ARGS] = [&[]; MAX_ARGS];
    for (i, p) in parts.iter().enumerate() {
        let n = p.len().min(ARG_LEN);
        arg_bufs[i][..n].copy_from_slice(&p[..n]);
    }
    for i in 0..parts.len() {
        let n = parts[i].len().min(ARG_LEN);
        arg_slices[i] = &arg_bufs[i][..n];
    }
    match fork() {
        Some(0) => {
            exec(path, &arg_slices[..parts.len()]);
            cmd_not_found(path, name);
            exit();
        }
        Some(_) => {
            let _ = wait();
        }
        None => write(b"fork failed\n"),
    }
}

fn split_args<'a>(line: &'a [u8], out: &mut [&'a [u8]; MAX_ARGS]) -> usize {
    let mut n = 0usize;
    let mut i = 0usize;
    while i < line.len() && n < MAX_ARGS {
        while i < line.len() && line[i] == b' ' {
            i += 1;
        }
        if i >= line.len() {
            break;
        }
        let start = i;
        while i < line.len() && line[i] != b' ' {
            i += 1;
        }
        out[n] = &line[start..i];
        n += 1;
    }
    n
}

fn command_path<'a>(name: &[u8], buf: &'a mut [u8]) -> &'a [u8] {
    let (start, src) = if name.first() == Some(&b'/') {
        (0, name)
    } else {
        buf[0] = b'/';
        (1, name)
    };
    let n = src.len().min(buf.len().saturating_sub(start));
    buf[start..start + n].copy_from_slice(&src[..n]);
    &buf[..start + n]
}

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    myos_user::panic_die(info);
}
