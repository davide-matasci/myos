#![no_main]

#[unsafe(no_mangle)]
pub extern "C" fn main() -> ! {
    std::process::exit(0);
}
