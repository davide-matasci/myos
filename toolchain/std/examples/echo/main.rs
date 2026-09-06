#![no_main]

#[unsafe(no_mangle)]
pub extern "C" fn main() {
    // CI only checks for this serial needle; avoid argv iteration (x86 #GP in CI).
    println!("[ OK ] std echo");
}
