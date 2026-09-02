//! x86_64: Limine already dropped us in long mode with HHDM + framebuffer.

pub mod gdt;
mod interrupts;
mod keyboard;
mod paging;
pub mod pci;
mod serial;
mod virtio_blk;
pub use serial::SerialPort;

pub fn serial_read_byte() -> Option<u8> {
    serial::read_byte()
}

pub fn serial_flush_rx() {
    serial::flush_rx();
}

pub fn keyboard_init() {
    keyboard::init();
}

pub fn keyboard_present() -> bool {
    keyboard::present()
}

pub fn keyboard_poll_byte() -> Option<u8> {
    keyboard::poll_byte()
}

pub const QEMU_SUCCESS: u32 = 0x10;
pub const QEMU_FAILURE: u32 = 0x11;

pub fn early_init() {}

pub fn init_interrupts() {
    interrupts::init();
}

pub fn wait_for_interrupt_proof() {
    interrupts::wait_for_interrupt_proof();
}

pub fn virtio_blk_init() {
    virtio_blk::init();
}

pub fn virtio_blk_count() -> u32 {
    virtio_blk::count()
}

pub fn virtio_blk_capacity(dev: u32) -> Option<u64> {
    virtio_blk::capacity(dev)
}

pub fn virtio_blk_read(dev: u32, lba: u64, buf: &mut [u8]) -> Result<(), ()> {
    virtio_blk::read(dev, lba, buf)
}

pub fn virtio_blk_write(dev: u32, lba: u64, buf: &[u8]) -> Result<(), ()> {
    virtio_blk::write(dev, lba, buf)
}

/// QEMU `isa-debug-exit` at iobase 0xf4. A no-op if the device was not added.
pub fn exit_qemu(code: u32) {
    unsafe {
        core::arch::asm!(
            "out dx, eax",
            in("dx") 0xf4_u16,
            in("eax") code,
            options(nomem, nostack, preserves_flags)
        );
    }
}

pub fn halt() -> ! {
    loop {
        unsafe {
            core::arch::asm!("hlt", options(nomem, nostack, preserves_flags));
        }
    }
}
