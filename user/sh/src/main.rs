#![no_std]
#![no_main]

use myos_user::{close, dup2, exec, exit, fork, open, pipe, read_line, wait_status, write};

const PROMPT: &[u8] = b"$ ";
const MAX_LINE: usize = 128;
const MAX_ARGS: usize = 8;
const ARG_LEN: usize = 32;

static mut LAST_STATUS: u8 = 0;

#[cfg(target_arch = "x86_64")]
#[unsafe(no_mangle)]
pub extern "C" fn _start() -> ! {
    shell()
}

#[cfg(target_arch = "aarch64")]
#[unsafe(no_mangle)]
pub extern "C" fn _start(argc: usize, argv: *const usize) -> ! {
    unsafe { myos_user::args::init_from_regs(argc, argv) };
    shell()
}

struct Segment<'a> {
    argc: usize,
    parts: [&'a [u8]; MAX_ARGS],
    in_path: Option<&'a [u8]>,
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
        let mut segs = [
            Segment {
                argc: 0,
                parts: [&[]; MAX_ARGS],
                in_path: None,
            },
            Segment {
                argc: 0,
                parts: [&[]; MAX_ARGS],
                in_path: None,
            },
        ];
        let nseg = parse_line(&line[..len], &mut segs);
        if nseg == 0 {
            continue;
        }
        if segs[0].argc == 1 && segs[0].parts[0] == b"exit" {
            exit();
        }
        if segs[0].argc == 1 && segs[0].parts[0] == b"$?" {
            print_status(unsafe { LAST_STATUS });
            write(b"\n");
            continue;
        }
        if nseg == 2 {
            run_pipeline(&segs[0], &segs[1]);
        } else {
            run_segment(&segs[0]);
        }
    }
}

fn print_status(code: u8) {
    if code >= 100 {
        write(&[b'0' + code / 100]);
    }
    if code >= 10 {
        write(&[b'0' + (code / 10) % 10]);
    }
    write(&[b'0' + code % 10]);
}

fn smoke_fork_ping() {
    match fork() {
        Some(0) => exit(),
        Some(_) => {
            let _ = wait_status();
            write(b"fork ok\n");
        }
        None => write(b"fork failed\n"),
    }
}

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
            let _ = wait_status();
            write(b"fork exec ok\n");
        }
        None => write(b"fork failed\n"),
    }
}

fn cmd_not_found(path: &[u8], cmd: &[u8]) {
    write(b"sh: command not found: ");
    write_bytes_escaped(path);
    write(b" (received: ");
    write_bytes_escaped(cmd);
    write(b")\n");
}

fn run_segment(seg: &Segment<'_>) {
    let mut path_buf = [0u8; 32];
    let mut arg_bufs = [[0u8; ARG_LEN]; MAX_ARGS];
    let mut arg_slices: [&[u8]; MAX_ARGS] = [&[]; MAX_ARGS];
    if seg.argc == 0 {
        return;
    }
    let path = command_path(seg.parts[0], &mut path_buf);
    for (i, p) in seg.parts[..seg.argc].iter().enumerate() {
        let n = p.len().min(ARG_LEN);
        arg_bufs[i][..n].copy_from_slice(&p[..n]);
    }
    for i in 0..seg.argc {
        let n = seg.parts[i].len().min(ARG_LEN);
        arg_slices[i] = &arg_bufs[i][..n];
    }
    match fork() {
        Some(0) => {
            apply_stdin_redir(seg.in_path);
            exec(path, &arg_slices[..seg.argc]);
            cmd_not_found(path, seg.parts[0]);
            exit();
        }
        Some(_) => {
            if let Some((_, code)) = wait_status() {
                unsafe { LAST_STATUS = code };
            }
        }
        None => write(b"fork failed\n"),
    }
}

fn run_pipeline(left: &Segment<'_>, right: &Segment<'_>) {
    let Some((rfd, wfd)) = pipe() else {
        write(b"sh: pipe failed\n");
        return;
    };
    let mut left_path = [0u8; 32];
    let mut right_path = [0u8; 32];
    let mut left_bufs = [[0u8; ARG_LEN]; MAX_ARGS];
    let mut right_bufs = [[0u8; ARG_LEN]; MAX_ARGS];
    let mut left_slices: [&[u8]; MAX_ARGS] = [&[]; MAX_ARGS];
    let mut right_slices: [&[u8]; MAX_ARGS] = [&[]; MAX_ARGS];
    if left.argc == 0 || right.argc == 0 {
        close(rfd);
        close(wfd);
        return;
    }
    let lpath = command_path(left.parts[0], &mut left_path);
    let rpath = command_path(right.parts[0], &mut right_path);
    for (i, p) in left.parts[..left.argc].iter().enumerate() {
        let n = p.len().min(ARG_LEN);
        left_bufs[i][..n].copy_from_slice(&p[..n]);
    }
    for (i, p) in right.parts[..right.argc].iter().enumerate() {
        let n = p.len().min(ARG_LEN);
        right_bufs[i][..n].copy_from_slice(&p[..n]);
    }
    for i in 0..left.argc {
        let n = left.parts[i].len().min(ARG_LEN);
        left_slices[i] = &left_bufs[i][..n];
    }
    for i in 0..right.argc {
        let n = right.parts[i].len().min(ARG_LEN);
        right_slices[i] = &right_bufs[i][..n];
    }
    match fork() {
        Some(0) => {
            close(rfd);
            dup2(wfd, 1);
            close(wfd);
            apply_stdin_redir(left.in_path);
            exec(lpath, &left_slices[..left.argc]);
            cmd_not_found(lpath, left.parts[0]);
            exit();
        }
        None => {
            close(rfd);
            close(wfd);
            write(b"fork failed\n");
            return;
        }
        Some(_left_pid) => {}
    }
    match fork() {
        Some(0) => {
            close(wfd);
            dup2(rfd, 0);
            close(rfd);
            apply_stdin_redir(right.in_path);
            exec(rpath, &right_slices[..right.argc]);
            cmd_not_found(rpath, right.parts[0]);
            exit();
        }
        None => {
            close(rfd);
            close(wfd);
            write(b"fork failed\n");
            return;
        }
        Some(_right_pid) => {}
    }
    close(rfd);
    close(wfd);
    let _ = wait_status();
    if let Some((_, code)) = wait_status() {
        unsafe { LAST_STATUS = code };
    }
}

fn apply_stdin_redir(path: Option<&[u8]>) {
    let Some(p) = path else {
        return;
    };
    let mut path_buf = [0u8; 64];
    let full = if p.first() == Some(&b'/') {
        p
    } else {
        path_buf[0] = b'/';
        let n = p.len().min(path_buf.len() - 1);
        path_buf[1..1 + n].copy_from_slice(&p[..n]);
        &path_buf[..1 + n]
    };
    let Some(fd) = open(full) else {
        write(b"sh: redirect open failed\n");
        exit();
    };
    dup2(fd, 0);
    close(fd);
}

fn parse_line<'a>(line: &'a [u8], segs: &mut [Segment<'a>; 2]) -> usize {
    let pipe_at = line.iter().position(|&b| b == b'|');
    let (left, right) = match pipe_at {
        Some(i) => (&line[..i], Some(&line[i + 1..])),
        None => (line, None),
    };
    segs[0] = parse_segment(left);
    if let Some(r) = right {
        segs[1] = parse_segment(r);
        if segs[0].argc > 0 && segs[1].argc > 0 {
            2
        } else {
            0
        }
    } else if segs[0].argc > 0 {
        1
    } else {
        0
    }
}

fn parse_segment<'a>(text: &'a [u8]) -> Segment<'a> {
    let mut seg = Segment {
        argc: 0,
        parts: [&[]; MAX_ARGS],
        in_path: None,
    };
    let mut tokens: [&[u8]; MAX_ARGS + 1] = [&[]; MAX_ARGS + 1];
    let mut nt = 0usize;
    let mut i = 0usize;
    while i < text.len() && nt < tokens.len() {
        while i < text.len() && text[i] == b' ' {
            i += 1;
        }
        if i >= text.len() {
            break;
        }
        let start = i;
        while i < text.len() && text[i] != b' ' {
            i += 1;
        }
        tokens[nt] = &text[start..i];
        nt += 1;
    }
    let mut j = 0usize;
    while j < nt && seg.argc < MAX_ARGS {
        if tokens[j] == b"<" {
            if j + 1 < nt {
                seg.in_path = Some(tokens[j + 1]);
                j += 2;
            } else {
                break;
            }
        } else {
            seg.parts[seg.argc] = tokens[j];
            seg.argc += 1;
            j += 1;
        }
    }
    seg
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
