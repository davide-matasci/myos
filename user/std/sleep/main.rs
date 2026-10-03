//! `thread::sleep` on the myos std: the task blocks in the kernel for the
//! duration, measured here by the wall clock (the `Instant` of this std has
//! no monotonic source yet).
#![no_main]

use std::time::{Duration, SystemTime};

#[unsafe(no_mangle)]
pub extern "C" fn main() {
    let want = Duration::from_millis(300);
    let start = SystemTime::now();
    std::thread::sleep(want);
    let slept = SystemTime::now().duration_since(start).unwrap_or(Duration::ZERO);
    // At least the duration; well under the seconds a stuck wait would take.
    if slept >= Duration::from_millis(250) && slept < Duration::from_secs(10) {
        println!("[ OK ] std sleep {} ms", slept.as_millis());
    } else {
        println!("[ FAIL ] std sleep: {} ms for a {} ms sleep", slept.as_millis(), want.as_millis());
        std::process::exit(1);
    }
}
