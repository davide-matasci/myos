//! Keyboard input on AArch64: virtio-input when present (QEMU/UTM), else none.

use super::virtio_input;

pub fn init() {
    virtio_input::init();
}

pub fn present() -> bool {
    virtio_input::present()
}

pub fn poll_byte() -> Option<u8> {
    virtio_input::poll_byte()
}
