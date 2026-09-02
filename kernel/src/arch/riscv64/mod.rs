//! RISC-V64: Limine on QEMU `virt` (UEFI). Sv39 MMU is already on.

mod interrupts;
mod keyboard;
pub mod paging;
mod serial;
mod virtio_blk;
mod virtio_input;
pub use serial::SerialPort;

pub fn serial_read_byte() -> Option<u8> {
    serial::read_byte()
}

pub fn serial_flush_rx() {}

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

/// Add device MMIO to Limine's Sv39 root when the low slot is still free.
pub fn early_init() {
    paging::map_devices();
}

pub fn init_interrupts() {
    interrupts::init();
}

pub fn wait_for_interrupt_proof() {
    interrupts::wait_for_interrupt_proof();
}

pub use interrupts::{fork_sret_child_to_user, fork_sret_to_user};

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

/// SBI System Reset extension shutdown (QEMU virt).
pub fn exit_qemu(_code: u32) {
    const SBI_SRST: u64 = 0x5352_5354; // "SRST"
    unsafe {
        core::arch::asm!(
            "ecall",
            in("a7") SBI_SRST,
            in("a6") 0u64,
            in("a0") 0u64,
            in("a1") 0u64,
            options(nomem, nostack),
        );
    }
}

pub fn halt() -> ! {
    loop {
        unsafe {
            core::arch::asm!("wfi", options(nomem, nostack, preserves_flags));
        }
    }
}
