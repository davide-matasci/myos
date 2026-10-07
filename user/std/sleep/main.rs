//! `thread::sleep` on the myos std: the task blocks in the kernel for the
//! duration, measured by both clocks: the wall clock (`SystemTime`) and the
//! monotonic one (`Instant`), which must agree.
#![no_main]

use std::time::{Duration, Instant, SystemTime};

#[unsafe(no_mangle)]
pub extern "C" fn main() {
    let want = Duration::from_millis(300);
    let start = SystemTime::now();
    let mono = Instant::now();
    std::thread::sleep(want);
    let slept = SystemTime::now().duration_since(start).unwrap_or(Duration::ZERO);
    let mono = mono.elapsed();
    // At least the duration; well under the seconds a stuck wait would take.
    let ok = |d: Duration| d >= Duration::from_millis(250) && d < Duration::from_secs(10);
    if ok(slept) && ok(mono) {
        println!("[ OK ] std sleep {} ms", slept.as_millis());
    } else {
        println!(
            "[ FAIL ] std sleep: {} ms (wall clock), {} ms (Instant) for a {} ms sleep",
            slept.as_millis(),
            mono.as_millis(),
            want.as_millis()
        );
        std::process::exit(1);
    }
}
