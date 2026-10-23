//! The Raspberry Pi's firmware (`docs/raspberry-pi.md`): the VideoCore runs
//! the board's power domains and clocks, and the ARM asks it through the
//! mailbox property interface (`brcm,bcm2835-mbox`, channel 8): a message
//! is a buffer of tags in memory the firmware reads and answers in place.
//! This module publishes that as the service [`RPI_FIRMWARE_SERVICE`] (a
//! [`RpiFirmwareOps`] table: `dwc2` powers the USB controller on through
//! it), and writes what the firmware says of the board to `/proc/firmware`.
//!
//! Speaks only through the module ABI. One message at a time, behind a
//! lock a call holds until the answer (task context only).

#![no_std]
#![no_main]
#![deny(unsafe_op_in_unsafe_fn)]

use core::fmt::Write;

use myos_abi::{
    ABI_VERSION, ApiCell, DMA_FROM_DEVICE, DMA_TO_DEVICE, DmaDevice, KernelApi, MmioDevice, RPI_CLOCK_ARM,
    RPI_CLOCK_CORE, RPI_CLOCK_EMMC, RPI_CLOCK_UART, RPI_FIRMWARE_SERVICE, RPI_FIRMWARE_VERSION, RPI_POWER_USB_HCD,
    RPI_TAG_BOARD_REVISION, RPI_TAG_FIRMWARE_REVISION, RPI_TAG_GET_POWER_STATE, RpiFirmwareOps, SleepLock,
    status_fail, status_ok,
};

/// The two mailboxes: the ARM reads 0, writes 1. Status bit 31: full, bit
/// 30: empty.
const MAIL0_READ: usize = 0x00;
const MAIL0_STATUS: usize = 0x18;
const MAIL1_WRITE: usize = 0x20;
const MAIL1_STATUS: usize = 0x38;
const STATUS_FULL: u32 = 1 << 31;
const STATUS_EMPTY: u32 = 1 << 30;
/// The property channel.
const CHANNEL: u32 = 8;
/// The message buffer: a page, so a message is at most its words.
const BUFFER_WORDS: usize = 1024;
/// How long the firmware may take.
const TIMEOUT_NS: u64 = 1_000_000_000;

/// The message buffer: `words` are the message as the firmware sees it
/// (size, request/response code, tags, end tag), at `bus` for it.
struct Mailbox {
    regs: usize,
    dma: i32,
    buf: *mut u32,
    bus: u64,
}

// SAFETY: the buffer is the module's own DMA page, used under the lock.
unsafe impl Send for Mailbox {}

static API: ApiCell = ApiCell::new();
static MAILBOX: SleepLock<Option<Mailbox>> = SleepLock::new(&API, None);

static OPS: RpiFirmwareOps = RpiFirmwareOps { version: RPI_FIRMWARE_VERSION, property };

fn api() -> &'static KernelApi {
    API.get()
}

fn read(regs: usize, off: usize) -> u32 {
    unsafe { core::ptr::read_volatile((regs + off) as *const u32) }
}

fn write(regs: usize, off: usize, value: u32) {
    unsafe { core::ptr::write_volatile((regs + off) as *mut u32, value) }
}

/// Spin until `done`, for at most `timeout_ns`: whether it was.
fn until(timeout_ns: u64, mut done: impl FnMut() -> bool) -> bool {
    let deadline = api().monotonic_ns() + timeout_ns;
    loop {
        if done() {
            return true;
        }
        if api().monotonic_ns() > deadline {
            return false;
        }
        core::hint::spin_loop();
    }
}

impl Mailbox {
    /// Run the message of `tags` (the tags alone): the answer over them. 0,
    /// or negative.
    fn call(&self, tags: &mut [u32]) -> i32 {
        // Size, code, the tags, the end tag.
        let words = tags.len() + 3;
        if words > BUFFER_WORDS {
            return -1;
        }
        let buf = unsafe { core::slice::from_raw_parts_mut(self.buf, words) };
        buf[0] = (words * 4) as u32;
        buf[1] = 0;
        buf[2..2 + tags.len()].copy_from_slice(tags);
        buf[words - 1] = 0;
        api().dma_sync(self.dma, self.buf as *const u8, words * 4, DMA_TO_DEVICE);
        if !until(TIMEOUT_NS, || read(self.regs, MAIL1_STATUS) & STATUS_FULL == 0) {
            return -2;
        }
        write(self.regs, MAIL1_WRITE, (self.bus as u32 & !0xf) | CHANNEL);
        // The answer names the buffer; another channel's message, were one
        // to come, is not ours.
        let mut answered = false;
        let ok = until(TIMEOUT_NS, || {
            if read(self.regs, MAIL0_STATUS) & STATUS_EMPTY != 0 {
                return false;
            }
            let v = read(self.regs, MAIL0_READ);
            answered = v & 0xf == CHANNEL && v & !0xf == self.bus as u32 & !0xf;
            answered
        });
        if !ok || !answered {
            return -2;
        }
        api().dma_sync(self.dma, self.buf as *const u8, words * 4, DMA_FROM_DEVICE);
        if buf[1] != 0x8000_0000 {
            return -3;
        }
        tags.copy_from_slice(&buf[2..2 + tags.len()]);
        0
    }
}

unsafe extern "C" fn property(tags: *mut u32, words: usize) -> i32 {
    if tags.is_null() || words == 0 {
        return -1;
    }
    let tags = unsafe { core::slice::from_raw_parts_mut(tags, words) };
    let mailbox = MAILBOX.lock();
    match mailbox.as_ref() {
        Some(m) => m.call(tags),
        None => -1,
    }
}

/// A text line, formatted without an allocator.
struct Line {
    buf: [u8; 256],
    len: usize,
}

impl Line {
    fn new() -> Line {
        Line { buf: [0; 256], len: 0 }
    }

    fn as_bytes(&self) -> &[u8] {
        &self.buf[..self.len]
    }
}

impl Write for Line {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        let n = s.len().min(self.buf.len() - self.len);
        self.buf[self.len..self.len + n].copy_from_slice(&s.as_bytes()[..n]);
        self.len += n;
        Ok(())
    }
}

/// `/proc/firmware`: the revisions, the clocks and the USB controller's
/// power, one per line, what the firmware answers.
fn proc_write(api: &KernelApi) {
    let mut text = Line::new();
    if let Some([rev]) = OPS.tag(RPI_TAG_FIRMWARE_REVISION, [0]) {
        let _ = writeln!(text, "firmware {rev:#x}");
    }
    if let Some([rev]) = OPS.tag(RPI_TAG_BOARD_REVISION, [0]) {
        let _ = writeln!(text, "board {rev:#x}");
    }
    for (name, clock) in [("emmc", RPI_CLOCK_EMMC), ("uart", RPI_CLOCK_UART), ("arm", RPI_CLOCK_ARM), ("core", RPI_CLOCK_CORE)] {
        if let Some(rate) = OPS.clock_rate(clock) {
            let _ = writeln!(text, "clock {name} {rate} Hz");
        }
    }
    if let Some([_, state]) = OPS.tag(RPI_TAG_GET_POWER_STATE, [RPI_POWER_USB_HCD, 0]) {
        let _ = writeln!(text, "power usb {}", if state & 1 != 0 { "on" } else { "off" });
    }
    api.proc_register("firmware", text.as_bytes());
}

#[inline(never)]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn module_init(api_ptr: *const KernelApi) -> i32 {
    if api_ptr.is_null() {
        return -1;
    }
    let api: &'static KernelApi = unsafe { &*api_ptr };
    if api.abi_version != ABI_VERSION {
        return -2;
    }
    unsafe { API.set(api) };
    let mut dev = MmioDevice::default();
    if api.dt_mmio_find("brcm,bcm2835-mbox", 0, &mut dev) != 0 {
        api.write_str("rpi_firmware none\n");
        return 0;
    }
    // The message buffer, where the firmware reaches it (the bus address
    // the tree's `dma-ranges` give: the GPU's uncached alias of memory).
    let dma = api.dma_attach(&DmaDevice::mmio(dev.phys));
    let mut bus = 0u64;
    let buf = api.dma_alloc(dma, 1, &mut bus);
    if dma < 0 || buf.is_null() {
        status_fail(api, "rpi_firmware: no DMA memory for the mailbox");
        return 0;
    }
    *MAILBOX.lock() = Some(Mailbox { regs: dev.base, dma, buf: buf as *mut u32, bus });
    if api.service_register(RPI_FIRMWARE_SERVICE, &OPS) != 0 {
        status_fail(api, "rpi_firmware: service");
        return -3;
    }
    proc_write(api);
    match OPS.tag(RPI_TAG_FIRMWARE_REVISION, [0]) {
        Some([rev]) => {
            let mut line = Line::new();
            let _ = write!(line, "rpi_firmware: revision {rev:#x}");
            status_ok(api, core::str::from_utf8(line.as_bytes()).unwrap_or("rpi_firmware"));
        }
        None => status_fail(api, "rpi_firmware: the firmware does not answer"),
    }
    0
}

#[inline(never)]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn module_exit() {}

// So `cargo build --bin rpi_firmware` links. The kernel never jumps here.
#[unsafe(no_mangle)]
pub extern "C" fn _start() -> ! {
    loop {}
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    loop {}
}
