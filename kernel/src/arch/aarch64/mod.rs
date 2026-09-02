//! AArch64: Limine on QEMU `virt` (UEFI). MMU is already on.

mod interrupts;
mod keyboard;
mod paging;
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

/// Identity-map device MMIO on TTBR0 so UART/GIC still work.
pub fn early_init() {
    paging::map_devices();
}

pub fn init_interrupts() {
    interrupts::init();
}

pub fn wait_for_interrupt_proof() {
    interrupts::wait_for_interrupt_proof();
}

pub use interrupts::fork_eret_to_user;

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

fn current_el() -> u64 {
    let el: u64;
    unsafe {
        core::arch::asm!("mrs {el}, CurrentEL", el = out(reg) el, options(nomem, nostack, preserves_flags));
    }
    (el >> 2) & 3
}

/// PSCI SYSTEM_OFF. EL1 uses HVC (QEMU virt conduit); EL2 uses SMC.
pub fn exit_qemu(_code: u32) {
    let cmd: u64 = 0x8400_0008;
    unsafe {
        if current_el() >= 2 {
            core::arch::asm!("smc #0", in("x0") cmd, options(nomem, nostack));
        } else {
            core::arch::asm!("hvc #0", in("x0") cmd, options(nomem, nostack));
        }
    }
}

pub fn halt() -> ! {
    loop {
        unsafe {
            core::arch::asm!("wfe", options(nomem, nostack, preserves_flags));
        }
    }
}
