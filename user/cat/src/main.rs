#![no_std]
#![no_main]

use myos_user::{close, exit, open, read, write};

#[cfg(target_arch = "x86_64")]
#[unsafe(no_mangle)]
pub extern "C" fn _start() -> ! {
    main()
}

#[cfg(target_arch = "aarch64")]
#[unsafe(no_mangle)]
pub extern "C" fn _start(argc: usize, argv: *const usize) -> ! {
    unsafe { myos_user::args::init_from_regs(argc, argv) };
    main()
}

fn main() -> ! {
    let path = match myos_user::arg(1) {
        Some(p) => p,
        None => {
            write(b"cat: missing file\n");
            exit();
        }
    };
    let mut path_buf = [0u8; 64];
    let full = if path.first() == Some(&b'/') {
        path
    } else {
        path_buf[0] = b'/';
        let n = path.len().min(path_buf.len() - 1);
        path_buf[1..1 + n].copy_from_slice(&path[..n]);
        &path_buf[..1 + n]
    };
    let Some(fd) = open(full) else {
        write(b"cat: open failed\n");
        exit();
    };
    let mut buf = [0u8; 128];
    loop {
        let n = read(fd, &mut buf);
        if n == usize::MAX || n == 0 {
            break;
        }
        write(&buf[..n]);
    }
    close(fd);
    exit();
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    loop {}
}
