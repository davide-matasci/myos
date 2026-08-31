#![no_main]

#[used]
static PADDING: [u8; 520_000] = [0; 520_000];

#[unsafe(no_mangle)]
pub extern "C" fn main() {
    // Smoke is pass/fail via exit status; heap prints "bigalloc ok". Avoid stdout
    // here — "big\n" during interactive shell CI (via `ok` → `/heap`) races serial
    // stdin and can drop the first typed character of the next command.
}
