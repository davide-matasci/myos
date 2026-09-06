#![no_std]
#![no_main]

use myos_user::dns::{format_ipv4, resolve_a, ResolveError};
use myos_user::{status_ok, exit, write};

myos_user::x86_start!(main);

#[cfg(any(target_arch = "aarch64", target_arch = "riscv64"))]
#[unsafe(no_mangle)]
pub extern "C" fn _start(argc: usize, argv: *const usize) -> ! {
    unsafe { myos_user::args::init_from_regs(argc, argv) };
    main()
}

fn usage() -> ! {
    write(b"usage: dns <hostname>\n");
    exit();
}

fn fail(msg: &[u8]) -> ! {
    write(msg);
    exit();
}

fn main() -> ! {
    let Some(host) = myos_user::arg(1) else {
        usage();
    };

    match resolve_a(host) {
        Ok(ip) => {
            write(b"IP: ");
            let mut ip_str = [0u8; 16];
            let n = format_ipv4(ip, &mut ip_str);
            write(&ip_str[..n]);
            write(b"\n");
            status_ok("dns");
            exit();
        }
        Err(ResolveError::Name) => fail(b"name too long\n"),
        Err(ResolveError::Open) => fail(b"open /net/udp fail\n"),
        Err(ResolveError::Clone) => fail(b"clone id fail\n"),
        Err(ResolveError::Connect) | Err(ResolveError::Timeout) => fail(b"udp connect timeout\n"),
        Err(ResolveError::Write) => fail(b"write query fail\n"),
        Err(ResolveError::NoResponse) | Err(ResolveError::NoARecord) => fail(b"dns no response\n"),
    }
}

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    myos_user::panic_die(info);
}
