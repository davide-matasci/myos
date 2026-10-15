//! virtio-net: `/dev/netN/`, Ethernet frames only; no IP.
//!
//! The device is `virtio_drivers`' raw net device (the driver owns the
//! buffers) over whichever transport `myos_virtio` found it on. `data`
//! reads never block (a `0` means "no frame"), but the RX queue raises an
//! interrupt (MSI-X on x86_64, the INTx line or the board interrupt
//! elsewhere) that wakes the pollers, so `netd` sleeps in `poll` on `data`
//! until a frame instead of polling the ring. `ctl` names the device's MAC
//! and whether that interrupt works (`mac 52:54:00:12:34:56`, `irq on|off`).

#![no_std]
#![no_main]
#![deny(unsafe_op_in_unsafe_fn)]

use core::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

use myos_abi::{ABI_VERSION, ApiCell, KernelApi, Lock, MYOS_POLLIN, MYOS_POLLOUT, ModuleChrOps, status_ok};
use myos_virtio::virtio_drivers::PAGE_SIZE;
use myos_virtio::virtio_drivers::device::net::VirtIONetRaw;
use myos_virtio::virtio_drivers::transport::DeviceType;
use myos_virtio::{Device, Id, IrqAck, IrqRoute, MyosHal};

const MAX_NET: usize = 4;
/// Entries per queue: the frames the device may hold for netd before it
/// drops them. A 64 KiB TCP window is 45 full frames; 16 lost most of a
/// burst, and every drop a retransmit. A device offering fewer entries is
/// not driven (QEMU offers 256).
const QSIZE: usize = 128;
/// One frame with its virtio header; the device's buffers are 2 KiB.
const BUF_SIZE: usize = 2048;
/// RX buffers come in runs of this many pages (`RX_CHUNK_BUFS` buffers
/// each): the contiguous allocation a module gets is a run of frames the
/// allocator happens to hand out in order, which 64 pages at once are not
/// sure to be.
const RX_CHUNK_PAGES: usize = 8;
const RX_CHUNK_BUFS: usize = RX_CHUNK_PAGES * PAGE_SIZE / BUF_SIZE;
const RX_CHUNKS: usize = QSIZE.div_ceil(RX_CHUNK_BUFS);
const RX_QUEUE: u16 = 0;
/// A virtqueue token that is no buffer's.
const NO_TOKEN: u16 = u16::MAX;

struct Net {
    id: Id,
    dev: VirtIONetRaw<MyosHal, Device, QSIZE>,
    /// The RX buffers, [`RX_CHUNK_BUFS`] per run (`rx_buf`), all posted to
    /// the device; `rx_token[i]` is what the queue calls buffer `i`.
    rx_chunks: [*mut u8; RX_CHUNKS],
    rx_token: [u16; QSIZE],
    tx_buf: *mut u8,
    /// A frame the device has not used yet (`write_frame`).
    tx_pending: u16,
    mac: [u8; 6],
    /// The RX queue raises interrupts (`irq_enable` succeeded).
    irq_ok: bool,
}

// SAFETY: the pointers are the device's own rings and buffers, used only
// by whoever holds its slot in `NETS`.
unsafe impl Send for Net {}

/// The devices, a lock per slot, held for a `data` read or write (one RX
/// and one TX buffer per device) or a look at `ctl`. The interrupt handler
/// never takes it: what it needs is in [`ACK`].
static NETS: [Lock<Option<Net>>; MAX_NET] = [const { Lock::new(None) }; MAX_NET];
/// Per slot, how the interrupt handler acknowledges the device
/// ([`IrqAck::to_usize`]); 0 before the device is up.
static ACK: [AtomicUsize; MAX_NET] = [const { AtomicUsize::new(0) }; MAX_NET];
/// Per slot, how many times `data`'s readiness may have changed (an
/// interrupt, a frame read): the change count its `poll` reports.
static CHANGES: [AtomicU64; MAX_NET] = [const { AtomicU64::new(0) }; MAX_NET];
static API: ApiCell = ApiCell::new();

macro_rules! chr_ops {
    ($n:literal, $read:ident, $write:ident, $poll:ident, $ctl:ident) => {
        unsafe extern "C" fn $read(buf: *mut u8, buf_len: usize) -> i32 {
            net_read_n($n, buf, buf_len)
        }
        unsafe extern "C" fn $write(buf: *const u8, buf_len: usize) -> i32 {
            net_write_n($n, buf, buf_len)
        }
        unsafe extern "C" fn $poll(changes: *mut u64) -> u32 {
            net_poll_n($n, changes)
        }
        unsafe extern "C" fn $ctl(buf: *mut u8, cap: usize) -> i32 {
            net_ctl_n($n, buf, cap)
        }
    };
}
chr_ops!(0, net0_read, net0_write, net0_poll, net0_ctl);
chr_ops!(1, net1_read, net1_write, net1_poll, net1_ctl);
chr_ops!(2, net2_read, net2_write, net2_poll, net2_ctl);
chr_ops!(3, net3_read, net3_write, net3_poll, net3_ctl);

static OPS: [ModuleChrOps; MAX_NET] = [
    ModuleChrOps { read: net0_read, write: net0_write, poll: Some(net0_poll), ctl_read: Some(net0_ctl), ctl_write: None },
    ModuleChrOps { read: net1_read, write: net1_write, poll: Some(net1_poll), ctl_read: Some(net1_ctl), ctl_write: None },
    ModuleChrOps { read: net2_read, write: net2_write, poll: Some(net2_poll), ctl_read: Some(net2_ctl), ctl_write: None },
    ModuleChrOps { read: net3_read, write: net3_write, poll: Some(net3_poll), ctl_read: Some(net3_ctl), ctl_write: None },
];

/// RX buffer `i`.
fn rx_buf(net: &Net, i: usize) -> &'static mut [u8] {
    let va = net.rx_chunks[i / RX_CHUNK_BUFS];
    // SAFETY: the chunk is `RX_CHUNK_BUFS` buffers of `BUF_SIZE`, never
    // freed; the slot lock serializes who touches buffer `i`.
    unsafe { core::slice::from_raw_parts_mut(va.add((i % RX_CHUNK_BUFS) * BUF_SIZE), BUF_SIZE) }
}

/// Hand buffer `i` to the device.
fn post_rx(net: &mut Net, i: usize) {
    let buf = rx_buf(net, i);
    // SAFETY: the buffer is read again only after `receive_complete`.
    net.rx_token[i] = unsafe { net.dev.receive_begin(buf) }.unwrap_or(NO_TOKEN);
}

fn probe_one(api: &KernelApi, slot_index: usize, id: Id, transport: Device) -> Option<Net> {
    let mut dev = match VirtIONetRaw::<MyosHal, Device, QSIZE>::new(transport) {
        Ok(d) => d,
        Err(_) => {
            api.write_str("virtio-net: a device did not come up\n");
            return None;
        }
    };
    let mac = dev.mac_address();

    let mut rx_chunks = [core::ptr::null_mut(); RX_CHUNKS];
    for chunk in rx_chunks.iter_mut() {
        *chunk = myos_virtio::dma_pages(RX_CHUNK_PAGES)?.1.as_ptr();
    }
    let tx_buf = myos_virtio::dma_pages(1)?.1.as_ptr();

    // RX interrupts: route the device's interrupt to `net_irq`; a PCI
    // function on x86_64 gets an MSI-X entry, which the queue has to be
    // pointed at. Failure leaves the device in poll mode; `ctl` says
    // `irq off` then, so netd keeps its timed polling.
    ACK[slot_index].store(IrqAck::of(id).to_usize(), Ordering::Release);
    let irq_ok = match myos_virtio::irq_enable(id, "virtio-net", net_irq, slot_index as *mut core::ffi::c_void) {
        Some(IrqRoute::Msix(entry)) => myos_virtio::set_queue_msix_vector(id, RX_QUEUE, entry),
        Some(IrqRoute::Line) => true,
        None => false,
    };
    if irq_ok {
        dev.enable_interrupts();
    } else {
        dev.disable_interrupts();
    }

    let mut net = Net { id, dev, rx_chunks, rx_token: [NO_TOKEN; QSIZE], tx_buf, tx_pending: NO_TOKEN, mac, irq_ok };
    for i in 0..QSIZE {
        post_rx(&mut net, i);
    }
    Some(net)
}

/// Run `f` on the device in slot `idx` with its lock held; `None` for an
/// empty slot.
fn with_net<R>(idx: usize, f: impl FnOnce(&mut Net) -> R) -> Option<R> {
    NETS.get(idx)?.lock().as_mut().map(f)
}

/// Interrupt handler: acknowledge the device (what deasserts a line;
/// harmless under MSI-X) and wake the pollers: `netd` sleeps in `poll` on
/// `data`. Atomics only: a handler must not wait for the slot's lock.
unsafe extern "C" fn net_irq(ctx: *mut core::ffi::c_void) {
    let idx = ctx as usize;
    IrqAck::from_usize(ACK.get(idx).map_or(0, |a| a.load(Ordering::Acquire))).ack();
    if let Some(c) = CHANGES.get(idx) {
        c.fetch_add(1, Ordering::SeqCst);
    }
    if let Some(api) = API.try_get() {
        api.wake_any();
    }
}

/// `poll` bits of `data`: readable when the RX ring holds a frame, always
/// writable; the slot's change count, read first.
fn net_poll_n(idx: usize, changes: *mut u64) -> u32 {
    // SAFETY: the kernel passes a live out-parameter.
    unsafe { *changes = CHANGES.get(idx).map_or(0, |c| c.load(Ordering::SeqCst)) };
    let readable = with_net(idx, |net| net.dev.poll_receive().is_some()).unwrap_or(false);
    if readable { MYOS_POLLIN | MYOS_POLLOUT } else { MYOS_POLLOUT }
}

/// The text of `ctl`: `mac 52:54:00:12:34:56` and `irq on|off` (whether the
/// RX queue interrupts, so a reader knows if `poll` on `data` wakes by itself).
fn net_ctl_n(idx: usize, buf: *mut u8, cap: usize) -> i32 {
    with_net(idx, |net| ctl_text(net, buf, cap)).unwrap_or(-1)
}

fn ctl_text(net: &Net, buf: *mut u8, cap: usize) -> i32 {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut text = [0u8; 32];
    let mut n = 0;
    for b in b"mac " {
        text[n] = *b;
        n += 1;
    }
    for (i, b) in net.mac.iter().enumerate() {
        if i != 0 {
            text[n] = b':';
            n += 1;
        }
        text[n] = HEX[(b >> 4) as usize];
        text[n + 1] = HEX[(b & 0xf) as usize];
        n += 2;
    }
    let irq: &[u8] = if net.irq_ok { b"\nirq on\n" } else { b"\nirq off\n" };
    for b in irq {
        text[n] = *b;
        n += 1;
    }
    if !buf.is_null() {
        let copy = n.min(cap);
        unsafe { core::ptr::copy_nonoverlapping(text.as_ptr(), buf, copy) };
    }
    n as i32
}

fn net_read_n(idx: usize, buf: *mut u8, buf_len: usize) -> i32 {
    if buf.is_null() {
        return -1;
    }
    let n = with_net(idx, |net| read_frame(net, buf, buf_len)).unwrap_or(-1);
    if n > 0 {
        if let Some(c) = CHANGES.get(idx) {
            c.fetch_add(1, Ordering::SeqCst);
        }
    }
    n
}

/// The next received frame into `buf`; 0 when none waits.
fn read_frame(net: &mut Net, buf: *mut u8, buf_len: usize) -> i32 {
    let Some(token) = net.dev.poll_receive() else {
        return 0;
    };
    let Some(i) = net.rx_token.iter().position(|&t| t == token) else {
        return 0;
    };
    let rx = rx_buf(net, i);
    // SAFETY: `rx` is the buffer `receive_begin` got for this token.
    let copy = match unsafe { net.dev.receive_complete(token, rx) } {
        Ok((hdr, len)) => {
            let copy = len.min(buf_len);
            // SAFETY: the kernel hands `buf_len` bytes at `buf`.
            unsafe { core::ptr::copy_nonoverlapping(rx.as_ptr().add(hdr), buf, copy) };
            copy
        }
        Err(_) => 0,
    };
    post_rx(net, i);
    copy as i32
}

fn net_write_n(idx: usize, buf: *const u8, buf_len: usize) -> i32 {
    if buf_len == 0 {
        return 0;
    }
    if buf.is_null() {
        return -1;
    }
    with_net(idx, |net| write_frame(net, buf, buf_len)).unwrap_or(-1)
}

/// Wait for the frame in flight, if any, at most `TX_SPIN` polls.
/// Syscalls run with interrupts masked (SIE clear on RISC-V): an unbounded
/// wait could freeze the guest for seconds when the used ring lagged. The
/// cap is well above normal QEMU completion (usually <<1k spins) but far
/// below a multi-second IRQ-off stall.
fn wait_tx_idle(net: &mut Net) -> bool {
    const TX_SPIN: u32 = 50_000;
    if net.tx_pending == NO_TOKEN {
        return true;
    }
    for _ in 0..TX_SPIN {
        if net.dev.poll_transmit() == Some(net.tx_pending) {
            let tx = unsafe { core::slice::from_raw_parts(net.tx_buf, BUF_SIZE) };
            // SAFETY: `tx` is the buffer `transmit_begin` got for this token.
            let _ = unsafe { net.dev.transmit_complete(net.tx_pending, tx) };
            net.tx_pending = NO_TOKEN;
            return true;
        }
        core::hint::spin_loop();
    }
    false
}

/// Send `buf` as one frame (cut to the buffer) and wait for the device to
/// take it; the bytes sent. A frame the device has not used within the
/// wait is reported as not sent but stays in flight: the next send waits
/// for it before it reuses the buffer.
fn write_frame(net: &mut Net, buf: *const u8, buf_len: usize) -> i32 {
    if !wait_tx_idle(net) {
        return -1;
    }
    // SAFETY: one page, ours; the device is done with it (`tx_pending`).
    let tx = unsafe { core::slice::from_raw_parts_mut(net.tx_buf, BUF_SIZE) };
    let Ok(hdr) = net.dev.fill_buffer_header(tx) else {
        return -1;
    };
    let frame = buf_len.min(BUF_SIZE - hdr);
    // SAFETY: the kernel hands `buf_len` bytes at `buf`.
    unsafe { core::ptr::copy_nonoverlapping(buf, tx.as_mut_ptr().add(hdr), frame) };
    // SAFETY: the buffer is reused only after `transmit_complete`.
    match unsafe { net.dev.transmit_begin(&tx[..hdr + frame]) } {
        Ok(token) => net.tx_pending = token,
        Err(_) => return -1,
    }
    if wait_tx_idle(net) { frame as i32 } else { -1 }
}

/// Whether the device is already up: a rescan must not reset it.
fn known(id: Id) -> bool {
    NETS.iter().any(|s| s.lock().as_ref().is_some_and(|n| n.id == id))
}

#[inline(never)]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn module_init(api: *const KernelApi) -> i32 {
    if api.is_null() {
        return -1;
    }
    let api: &'static KernelApi = unsafe { &*api };
    if api.abi_version != ABI_VERSION {
        return -2;
    }
    unsafe {
        API.set(api);
        myos_virtio::init(api);
    }
    const NAMES: [&str; MAX_NET] = ["net0", "net1", "net2", "net3"];
    let mut registered = 0usize;
    myos_virtio::probe(DeviceType::Network, &mut known, &mut |id, transport| {
        if registered >= MAX_NET {
            return;
        }
        let slot = registered;
        let Some(net) = probe_one(api, slot, id, transport) else {
            return;
        };
        *NETS[slot].lock() = Some(net);
        if api.dev_register(NAMES[slot], &OPS[slot]) != 0 {
            *NETS[slot].lock() = None;
            return;
        }
        registered += 1;
    });
    if registered == 0 {
        api.write_str("virtio-net skip\n");
    } else {
        status_ok(api, "virtio-net");
    }
    0
}

#[inline(never)]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn module_exit() {}

#[unsafe(no_mangle)]
pub extern "C" fn _start() -> ! {
    loop {}
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    loop {}
}
