//! Keyboard input on AArch64/RISC-V: virtio-input when present (QEMU/UTM), else none.

use super::input as virtio_input;

pub fn init() {
    virtio_input::init();
}

pub fn present() -> bool {
    virtio_input::present()
}

pub fn poll_byte() -> Option<u8> {
    virtio_input::poll_byte()
}
