//! No keyboard driver on AArch64 yet (USB HID is out of scope for this pass).

pub fn init() {}

pub fn present() -> bool {
    false
}

pub fn poll_byte() -> Option<u8> {
    None
}
