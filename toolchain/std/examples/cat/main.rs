#![feature(myos_ext)]
#![no_main]

use std::fs::File;
use std::io::{self, Read};
use std::path::Path;

#[unsafe(no_mangle)]
pub extern "C" fn main() {
    if let Err(_e) = run() {}
}

#[inline(never)]
fn run() -> io::Result<()> {
    let mut file = File::open(Path::new("/msg"))?;
    let mut buf = [0u8; 8];
    let n = file.read(&mut buf)?;
    if n == 0 {
        return Err(io::ErrorKind::UnexpectedEof.into());
    }
    println!("std cat ok");
    Ok(())
}
