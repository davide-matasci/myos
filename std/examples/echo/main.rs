#![feature(myos_ext)]
#![no_main]

use std::os::myos::args;
use std::os::myos::ffi::OsStrExt;

#[unsafe(no_mangle)]
pub extern "C" fn main() {
    emit_args();
    println!("std echo ok");
}

#[inline(never)]
fn emit_args() {
    let mut first = true;
    for arg in args::args_os().skip(1) {
        if !first {
            print!(" ");
        }
        for byte in arg.as_bytes() {
            print!("{}", *byte as char);
        }
        first = false;
    }
    if !first {
        println!();
    }
}
