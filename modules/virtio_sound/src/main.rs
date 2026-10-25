//! virtio-sound (virtio 1.2, device 25): a card's PCM streams, playback
//! and capture, for the sound core (`docs/sound.md`).
//!
//! The driver is its own (the `virtio_drivers` one busy-waits for the
//! length of the audio and plays only): the crate's transport and split
//! queues, the device's four queues (control, event, tx, rx; the event
//! queue is not driven). A stream the core starts gets its parameters
//! (`SET_PARAMS`, `PREPARE`, `START`) and [`DEPTH`] periods in flight on
//! its queue, each a header naming the stream, the PCM and the device's
//! status: as one comes back the card's thread refills it from the core
//! (playback, [`SoundOps::playback_take`]) or hands its PCM over and posts
//! it again (capture, [`SoundOps::capture_put`]). The device's interrupt
//! wakes the thread; without one it looks every few milliseconds.

#![no_std]
#![no_main]
#![deny(unsafe_op_in_unsafe_fn)]

use core::fmt::Write;
use core::sync::atomic::{AtomicPtr, AtomicU8, AtomicUsize, Ordering};

use myos_abi::{
    status_ok, ApiCell, KernelApi, Lock, SoundCardOps, SoundOps, SoundParams, SoundStreamInfo, ABI_VERSION,
    SOUND_CAPTURE, SOUND_FORMAT_S16, SOUND_FORMAT_U8, SOUND_PLAYBACK, SOUND_RATES, SOUND_SERVICE, SOUND_VERSION,
};
use myos_virtio::virtio_drivers::device::common::Feature;
use myos_virtio::virtio_drivers::queue::VirtQueue;
use myos_virtio::virtio_drivers::transport::{DeviceType, Transport};
use myos_virtio::virtio_drivers::PAGE_SIZE;
use myos_virtio::{Device, Id, IrqAck, IrqRoute, MyosHal};

const MAX_CARDS: usize = 4;
/// Streams driven per card (QEMU's device has one of each direction).
const MAX_STREAMS: usize = 4;
const QSIZE: usize = 32;
/// Periods in flight per running stream.
const DEPTH: usize = 4;
/// The longest period the driver takes (the core offers up to 64 KiB).
const PERIOD_MAX: usize = 16 * 1024;
/// Polls of a control request before the device is given up on: QEMU
/// answers at once, a syscall waits with interrupts masked.
const CONTROL_SPIN: u32 = 5_000_000;

const CONTROL_QUEUE: u16 = 0;
const TX_QUEUE: u16 = 2;
const RX_QUEUE: u16 = 3;

// Requests (virtio 1.2, 5.14.6).
const R_PCM_INFO: u32 = 0x0100;
const R_PCM_SET_PARAMS: u32 = 0x0101;
const R_PCM_PREPARE: u32 = 0x0102;
const R_PCM_RELEASE: u32 = 0x0103;
const R_PCM_START: u32 = 0x0104;
const R_PCM_STOP: u32 = 0x0105;
const S_OK: u32 = 0x8000;
/// `VIRTIO_SND_D_*` and `VIRTIO_SND_PCM_FMT_*`.
const D_INPUT: u8 = 1;
const FMT_U8: u8 = 4;
const FMT_S16: u8 = 5;
/// `struct virtio_snd_pcm_info`.
const PCM_INFO_SIZE: usize = 32;
/// The device's status after a transfer's PCM (`status`, `latency_bytes`).
const XFER_STATUS: usize = 8;

/// A stream's buffers: [`DEPTH`] periods of PCM, and a page of their
/// headers (16 bytes each from 0) and statuses (from 512).
struct Stream {
    /// The device's number for it.
    id: u32,
    info: SoundStreamInfo,
    data: *mut u8,
    meta: *mut u8,
    /// How it runs, while it does.
    run: Option<SoundParams>,
    /// What the queue calls each period in flight.
    tokens: [Option<u16>; DEPTH],
}

struct Card {
    id: Id,
    transport: Device,
    control: VirtQueue<MyosHal, QSIZE>,
    tx: VirtQueue<MyosHal, QSIZE>,
    rx: VirtQueue<MyosHal, QSIZE>,
    /// A page for control requests (from 0) and their answers (from 2048).
    ctl: *mut u8,
    streams: [Option<Stream>; MAX_STREAMS],
    /// The core's number for the card.
    card: i32,
}

// SAFETY: the pointers are the card's own DMA pages, used only by whoever
// holds its slot in `CARDS`.
unsafe impl Send for Card {}

static CARDS: [Lock<Option<Card>>; MAX_CARDS] = [const { Lock::new(None) }; MAX_CARDS];
/// Per card, how its interrupt handler acknowledges the device.
static ACK: [AtomicUsize; MAX_CARDS] = [const { AtomicUsize::new(0) }; MAX_CARDS];
/// Per card, the key its thread waits on (the address).
static KEYS: [AtomicU8; MAX_CARDS] = [const { AtomicU8::new(0) }; MAX_CARDS];
static CORE: AtomicPtr<SoundOps> = AtomicPtr::new(core::ptr::null_mut());
static API: ApiCell = ApiCell::new();

fn api() -> &'static KernelApi {
    API.get()
}

fn sound() -> &'static SoundOps {
    // SAFETY: set in `module_init` before any card is registered.
    unsafe { &*CORE.load(Ordering::Acquire) }
}

fn key(slot: usize) -> usize {
    &KEYS[slot] as *const AtomicU8 as usize
}

fn page(va: *mut u8, off: usize, len: usize) -> &'static mut [u8] {
    // SAFETY: inside one of the card's DMA allocations, which are never
    // freed; the slot lock serializes who touches them.
    unsafe { core::slice::from_raw_parts_mut(va.add(off), len) }
}

fn put32(buf: &mut [u8], off: usize, v: u32) {
    buf[off..off + 4].copy_from_slice(&v.to_le_bytes());
}

fn get32(buf: &[u8], off: usize) -> u32 {
    u32::from_le_bytes(buf[off..off + 4].try_into().unwrap())
}

/// Send the `len`-byte request in the control page and wait for the
/// `answer`-byte reply after it: the reply's status code, `None` when the
/// device did not answer.
fn control(card: &mut Card, len: usize, answer: usize) -> Option<u32> {
    let req: &[u8] = page(card.ctl, 0, len);
    let rsp = page(card.ctl, 2048, answer);
    let inputs = [req];
    let mut outputs = [rsp];
    // SAFETY: the buffers stay put until popped below (or forever, if the
    // device never answers: the page is not reused then).
    let token = unsafe { card.control.add(&inputs, &mut outputs) }.ok()?;
    card.transport.notify(CONTROL_QUEUE);
    let mut spins = 0;
    while !card.control.can_pop() {
        spins += 1;
        if spins > CONTROL_SPIN {
            return None;
        }
        core::hint::spin_loop();
    }
    // SAFETY: the same buffers `add` got.
    unsafe { card.control.pop_used(token, &inputs, &mut outputs) }.ok()?;
    Some(get32(page(card.ctl, 2048, 4), 0))
}

/// A request naming stream `id` (`struct virtio_snd_pcm_hdr`): its status.
fn pcm_request(card: &mut Card, code: u32, id: u32) -> Option<u32> {
    let req = page(card.ctl, 0, 8);
    put32(req, 0, code);
    put32(req, 4, id);
    control(card, 8, 4)
}

/// The device's streams: `(id, what it can do)`, the ones the core can
/// drive (U8 or S16 at a rate it names).
fn query_streams(card: &mut Card, count: u32, out: &mut [(u32, SoundStreamInfo); MAX_STREAMS]) -> usize {
    let count = count.min(MAX_STREAMS as u32);
    let req = page(card.ctl, 0, 16);
    put32(req, 0, R_PCM_INFO);
    put32(req, 4, 0);
    put32(req, 8, count);
    put32(req, 12, PCM_INFO_SIZE as u32);
    if control(card, 16, 4 + count as usize * PCM_INFO_SIZE) != Some(S_OK) {
        return 0;
    }
    let mut n = 0;
    for i in 0..count as usize {
        let info = page(card.ctl, 2048 + 4 + i * PCM_INFO_SIZE, PCM_INFO_SIZE);
        let formats = u64::from_le_bytes(info[8..16].try_into().unwrap());
        let rates = u64::from_le_bytes(info[16..24].try_into().unwrap());
        let mut ours = 0u8;
        if formats & 1 << FMT_U8 != 0 {
            ours |= 1 << SOUND_FORMAT_U8;
        }
        if formats & 1 << FMT_S16 != 0 {
            ours |= 1 << SOUND_FORMAT_S16;
        }
        // The core's rate bits are the device's (`VIRTIO_SND_PCM_RATE_*`).
        let rates = (rates & ((1 << SOUND_RATES.len()) - 1)) as u32;
        if ours == 0 || rates == 0 {
            continue;
        }
        let direction = if info[24] == D_INPUT { SOUND_CAPTURE } else { SOUND_PLAYBACK };
        out[n] = (
            i as u32,
            SoundStreamInfo { direction, channels_min: info[25].max(1), channels_max: info[26].max(1), formats: ours, rates },
        );
        n += 1;
    }
    n
}

/// Post period `k` of stream `s` on its queue, the PCM filled first for
/// playback (silence for what the core does not have).
fn post(card: &mut Card, s: usize, k: usize) {
    let core_card = card.card;
    let Some(st) = card.streams[s].as_mut() else {
        return;
    };
    let Some(p) = st.run else {
        return;
    };
    let period = p.period_bytes as usize;
    let hdr = page(st.meta, k * 16, 4);
    put32(hdr, 0, st.id);
    let pcm = page(st.data, k * PERIOD_MAX, period);
    let status = page(st.meta, 512 + k * 16, XFER_STATUS);
    let token = if st.info.direction == SOUND_PLAYBACK {
        let got = sound().playback_take(core_card, s as u32, pcm);
        let silence = if p.format == SOUND_FORMAT_U8 { 0x80 } else { 0 };
        pcm[got..].fill(silence);
        let inputs = [&*hdr, &*pcm];
        // SAFETY: the buffers are touched again only once popped (`pump`).
        unsafe { card.tx.add(&inputs, &mut [status]) }
    } else {
        // SAFETY: as above.
        unsafe { card.rx.add(&[&*hdr], &mut [pcm, status]) }
    };
    st.tokens[k] = token.ok();
}

/// Take back what the device used on both queues: refill the playback
/// periods, hand the captured ones to the core and post them again.
/// Whether a stream runs.
fn pump(card: &mut Card) -> bool {
    for capture in [false, true] {
        loop {
            let queue = if capture { &card.rx } else { &card.tx };
            let Some(token) = queue.peek_used() else {
                break;
            };
            let found = card.streams.iter().enumerate().find_map(|(s, st)| {
                let st = st.as_ref()?;
                st.tokens.iter().position(|t| *t == Some(token)).map(|k| (s, k))
            });
            let Some((s, k)) = found else {
                break;
            };
            let st = card.streams[s].as_mut().unwrap();
            let period = st.run.map_or(PERIOD_MAX, |p| p.period_bytes as usize);
            let hdr: &[u8] = page(st.meta, k * 16, 4);
            let pcm = page(st.data, k * PERIOD_MAX, period);
            let status = page(st.meta, 512 + k * 16, XFER_STATUS);
            st.tokens[k] = None;
            // SAFETY: the buffers `post` gave the queue for this token.
            let used = unsafe {
                if capture {
                    card.rx.pop_used(token, &[hdr], &mut [&mut *pcm, status])
                } else {
                    card.tx.pop_used(token, &[hdr, &*pcm], &mut [status])
                }
            };
            let running = st.run.is_some();
            if capture && running {
                if let Ok(len) = used {
                    let got = (len as usize).saturating_sub(XFER_STATUS).min(period);
                    sound().capture_put(card.card, s as u32, &pcm[..got]);
                }
            }
            if running {
                post(card, s, k);
            }
        }
    }
    let running = card.streams.iter().flatten().any(|s| s.run.is_some());
    if card.tx.should_notify() {
        card.transport.notify(TX_QUEUE);
    }
    if card.rx.should_notify() {
        card.transport.notify(RX_QUEUE);
    }
    running
}

/// The card's thread: pump while a stream runs, woken by the interrupt.
unsafe extern "C" fn card_thread(ctx: *mut core::ffi::c_void) {
    let slot = ctx as usize;
    let api = api();
    loop {
        let seq = api.wait_seq();
        let running = CARDS[slot].lock().as_mut().is_some_and(pump);
        let wait = if running { 5_000_000 } else { 200_000_000 };
        api.block_until(key(slot), seq, api.monotonic_ns() + wait);
    }
}

/// The device's interrupt: acknowledge it and wake the card's thread.
unsafe extern "C" fn card_irq(ctx: *mut core::ffi::c_void) {
    let slot = ctx as usize;
    IrqAck::from_usize(ACK.get(slot).map_or(0, |a| a.load(Ordering::Acquire))).ack();
    if let Some(api) = API.try_get() {
        api.wake(key(slot));
    }
}

// ---- The core's calls --------------------------------------------------------

static CARD_OPS: SoundCardOps = SoundCardOps { version: SOUND_VERSION, start: card_start, stop: card_stop };

unsafe extern "C" fn card_start(ctx: usize, stream: u32, params: *const SoundParams) -> i32 {
    // SAFETY: the core hands its parameters.
    let Some(p) = (unsafe { params.as_ref() }).copied() else {
        return -1;
    };
    let Some(rate) = SOUND_RATES.iter().position(|&r| r == p.rate) else {
        return -1;
    };
    if p.period_bytes as usize > PERIOD_MAX || p.period_bytes == 0 {
        return -1;
    }
    let Some(slot) = CARDS.get(ctx) else {
        return -1;
    };
    let mut guard = slot.lock();
    let Some(card) = guard.as_mut() else {
        return -1;
    };
    let Some(id) = card.streams.get(stream as usize).and_then(Option::as_ref).map(|s| s.id) else {
        return -1;
    };
    let req = page(card.ctl, 0, 24);
    put32(req, 0, R_PCM_SET_PARAMS);
    put32(req, 4, id);
    put32(req, 8, p.period_bytes * DEPTH as u32);
    put32(req, 12, p.period_bytes);
    put32(req, 16, 0);
    req[20] = p.channels;
    req[21] = if p.format == SOUND_FORMAT_S16 { FMT_S16 } else { FMT_U8 };
    req[22] = rate as u8;
    req[23] = 0;
    if control(card, 24, 4) != Some(S_OK) || pcm_request(card, R_PCM_PREPARE, id) != Some(S_OK) {
        return -1;
    }
    card.streams[stream as usize].as_mut().unwrap().run = Some(p);
    for k in 0..DEPTH {
        post(card, stream as usize, k);
    }
    card.transport.notify(TX_QUEUE);
    card.transport.notify(RX_QUEUE);
    if pcm_request(card, R_PCM_START, id) != Some(S_OK) {
        card.streams[stream as usize].as_mut().unwrap().run = None;
        stop_stream(card, stream as usize);
        return -1;
    }
    drop(guard);
    api().wake(key(ctx));
    0
}

unsafe extern "C" fn card_stop(ctx: usize, stream: u32) {
    let Some(slot) = CARDS.get(ctx) else {
        return;
    };
    if let Some(card) = slot.lock().as_mut() {
        if let Some(st) = card.streams.get_mut(stream as usize).and_then(Option::as_mut) {
            st.run = None;
        }
        stop_stream(card, stream as usize);
    }
}

/// Stop the device's stream and release its buffers: the device gives
/// back every period in flight, which `pump` takes (no longer refilling).
fn stop_stream(card: &mut Card, s: usize) {
    let Some(id) = card.streams[s].as_ref().map(|st| st.id) else {
        return;
    };
    pcm_request(card, R_PCM_STOP, id);
    pcm_request(card, R_PCM_RELEASE, id);
    let mut spins = 0;
    while card.streams[s].as_ref().is_some_and(|st| st.tokens.iter().any(Option::is_some)) && spins < CONTROL_SPIN {
        pump(card);
        spins += 1;
        core::hint::spin_loop();
    }
}

// ---- Bringing a device up ----------------------------------------------------

/// The card's name for the core (`virtio-sound pci 00:04.0`).
struct Name([u8; 40], usize);

impl Write for Name {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        for b in s.bytes() {
            if self.1 < self.0.len() {
                self.0[self.1] = b;
                self.1 += 1;
            }
        }
        Ok(())
    }
}

fn probe_one(api: &KernelApi, slot: usize, id: Id, mut transport: Device) -> Option<Card> {
    transport.begin_init(Feature::VERSION_1);
    let control = VirtQueue::new(&mut transport, CONTROL_QUEUE, false, false).ok()?;
    let tx = VirtQueue::new(&mut transport, TX_QUEUE, false, false).ok()?;
    let rx = VirtQueue::new(&mut transport, RX_QUEUE, false, false).ok()?;
    transport.finish_init();
    // `struct virtio_snd_config`: jacks, streams, chmaps.
    let count: u32 = transport.read_config_space(4).ok()?;
    let ctl = myos_virtio::dma_pages(1)?.1.as_ptr();
    let mut card = Card { id, transport, control, tx, rx, ctl, streams: [const { None }; MAX_STREAMS], card: -1 };
    let mut found = [(0, SoundStreamInfo::default()); MAX_STREAMS];
    let n = query_streams(&mut card, count, &mut found);
    for (i, &(sid, info)) in found[..n].iter().enumerate() {
        let data = myos_virtio::dma_pages(DEPTH * PERIOD_MAX / PAGE_SIZE)?.1.as_ptr();
        let meta = myos_virtio::dma_pages(1)?.1.as_ptr();
        card.streams[i] = Some(Stream { id: sid, info, data, meta, run: None, tokens: [None; DEPTH] });
    }
    if n == 0 {
        api.write_str("virtio-sound: a device without a stream the driver takes\n");
        return None;
    }
    // The used buffers raise the interrupt (one MSI-X entry for all the
    // queues); without it the thread polls.
    ACK[slot].store(IrqAck::of(id).to_usize(), Ordering::Release);
    if let Some(IrqRoute::Msix(entry)) = myos_virtio::irq_enable(id, "virtio-sound", card_irq, slot as *mut core::ffi::c_void) {
        for q in [CONTROL_QUEUE, TX_QUEUE, RX_QUEUE] {
            myos_virtio::set_queue_msix_vector(id, q, entry);
        }
    }
    Some(card)
}

/// Whether the device is already up: a rescan must not reset it.
fn known(id: Id) -> bool {
    CARDS.iter().any(|s| s.lock().as_ref().is_some_and(|c| c.id == id))
}

fn probe_all(api: &'static KernelApi) -> usize {
    let mut up = 0;
    myos_virtio::probe(DeviceType::Sound, &mut known, &mut |id, transport| {
        let Some(slot) = CARDS.iter().position(|s| s.lock().is_none()) else {
            return;
        };
        let Some(card) = probe_one(api, slot, id, transport) else {
            return;
        };
        let mut infos = [SoundStreamInfo::default(); MAX_STREAMS];
        let mut n = 0;
        for st in card.streams.iter().flatten() {
            infos[n] = st.info;
            n += 1;
        }
        let mut name = Name([0; 40], 0);
        let _ = write!(name, "virtio-sound {id}");
        *CARDS[slot].lock() = Some(card);
        let number = sound().card_register(&CARD_OPS, slot, core::str::from_utf8(&name.0[..name.1]).unwrap_or("virtio-sound"), &infos[..n]);
        if number < 0 {
            api.write_str("virtio-sound: the sound core has no room for a card\n");
            return;
        }
        if let Some(c) = CARDS[slot].lock().as_mut() {
            c.card = number;
        }
        if api.thread_spawn("virtio-sound", card_thread, slot as *mut core::ffi::c_void) < 0 {
            api.write_str("virtio-sound: no thread for a card\n");
            return;
        }
        up += 1;
    });
    up
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
    let core = api.service_lookup(SOUND_SERVICE) as *mut SoundOps;
    // SAFETY: the sound module's table, loaded before this one.
    if core.is_null() || unsafe { (*core).version } != SOUND_VERSION {
        api.write_str("virtio-sound: no sound core\n");
        return -3;
    }
    CORE.store(core, Ordering::Release);
    if probe_all(api) == 0 {
        api.write_str("virtio-sound skip\n");
    } else {
        status_ok(api, "virtio-sound");
    }
    0
}

/// Probe for devices that appeared since `module_init`.
#[inline(never)]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn module_rescan() {
    if let Some(api) = API.try_get() {
        probe_all(api);
    }
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
