#![no_main]

#[used]
static PADDING: [u8; 520_000] = [0; 520_000];

#[unsafe(no_mangle)]
pub extern "C" fn main() {
    let mut v = Vec::with_capacity(4);
    v.push(b'b');
    v.push(b'i');
    v.push(b'g');
    v.push(b'\n');
    let _ = std::io::Write::write_all(&mut std::io::stdout(), &v);
}
