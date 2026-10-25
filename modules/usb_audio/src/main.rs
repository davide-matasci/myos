//! USB Audio Class 1 playback (a speaker, QEMU's `usb-audio`): the
//! audio streaming interface's isochronous OUT endpoint, as a sound card
//! of the sound core (`docs/sound.md`).
//!
//! The driver takes a device's audio control interface (nothing to do
//! there) and its streaming interface, whose alternate setting with an
//! isochronous OUT endpoint and a type I format of 16-bit PCM it plays
//! through: the card's one playback stream, its channels and the rates the
//! format lists. A started stream selects that setting (the host gives the
//! endpoint its bandwidth), sets the endpoint's sampling frequency and
//! keeps [`AHEAD`] packets of 1 ms queued: as packets complete, the host's
//! thread calls back and the driver queues the next ones from the core
//! (silence for what it does not have). Stopping goes back to setting 0.

#![no_std]
#![no_main]
#![deny(unsafe_op_in_unsafe_fn)]

use core::ffi::c_void;
use core::fmt::Write;
use core::sync::atomic::{AtomicPtr, Ordering};

use myos_abi::{
    status_fail, status_ok, ApiCell, KernelApi, SleepLock, SoundCardOps, SoundOps, SoundParams, SoundStreamInfo,
    StrRef, UsbDeviceInfo, UsbDriverOps, UsbHostOps, UsbInterfaceInfo, ABI_VERSION, SOUND_FORMAT_S16, SOUND_PLAYBACK,
    SOUND_RATES, SOUND_SERVICE, SOUND_VERSION, USB_HOST_VERSION, USB_ISOCH_PACKET_MAX, USB_SERVICE,
};

const MAX_CARDS: usize = 4;
const CLASS_AUDIO: u8 = 1;
const SUBCLASS_CONTROL: u8 = 1;
const SUBCLASS_STREAMING: u8 = 2;
/// Class-specific interface descriptor and its subtype of a format.
const CS_INTERFACE: u8 = 0x24;
const AS_FORMAT_TYPE: u8 = 2;
const FORMAT_TYPE_I: u8 = 1;
/// Packets queued at a time, and how many go at once.
const AHEAD: u32 = 24;
const BATCH: usize = 8;

static API: ApiCell = ApiCell::new();
static HOST: AtomicPtr<UsbHostOps> = AtomicPtr::new(core::ptr::null_mut());
static SOUND: AtomicPtr<SoundOps> = AtomicPtr::new(core::ptr::null_mut());

fn api() -> &'static KernelApi {
    API.get()
}

fn host() -> &'static UsbHostOps {
    // SAFETY: set in `module_init` before the driver is registered.
    unsafe { &*HOST.load(Ordering::Acquire) }
}

fn sound() -> &'static SoundOps {
    // SAFETY: as `host`.
    unsafe { &*SOUND.load(Ordering::Acquire) }
}

/// A running stream: its rate and frame size, the fraction of a frame
/// each packet carries over (44.1 kHz: 44 or 45 frames a packet), and the
/// packets queued and completed so far.
struct Run {
    rate: u32,
    frame: usize,
    carry: u32,
    queued: u32,
    done: u32,
}

struct Card {
    dev: u32,
    interface: u8,
    alternate: u8,
    endpoint: u8,
    max_packet: usize,
    channels: u8,
    /// The core's number for the card.
    card: i32,
    run: Option<Run>,
}

static CARDS: [SleepLock<Option<Card>>; MAX_CARDS] = [const { SleepLock::new(&API, None) }; MAX_CARDS];

/// What a streaming interface offers: the alternate setting, its
/// isochronous OUT endpoint (address, max packet), the channels and the
/// rates (`SOUND_RATES` bits) of its 16-bit PCM format.
struct Setting {
    alternate: u8,
    endpoint: u8,
    max_packet: usize,
    channels: u8,
    rates: u32,
}

/// The device's configuration descriptor into `buf`: its length.
fn config_descriptor(dev: u32, buf: &mut [u8]) -> usize {
    let n = host().control(dev, 0x80, 6, 0x0200, 0, &mut buf[..9]);
    if n < 9 {
        return 0;
    }
    let total = usize::from(u16::from_le_bytes([buf[2], buf[3]])).min(buf.len());
    let n = host().control(dev, 0x80, 6, 0x0200, 0, &mut buf[..total]);
    if n < total as i32 { 0 } else { total }
}

/// The first alternate setting of `interface` in `cfg` this driver plays
/// through.
fn find_setting(cfg: &[u8], interface: u8) -> Option<Setting> {
    let mut i = 0usize;
    let mut cur: Option<Setting> = None;
    let mut found: Option<Setting> = None;
    let mut take = |s: Option<Setting>, found: &mut Option<Setting>| {
        if let Some(s) = s.filter(|s| s.endpoint != 0 && s.rates != 0 && s.channels != 0) {
            found.get_or_insert(s);
        }
    };
    while i + 2 <= cfg.len() {
        let len = usize::from(cfg[i]);
        if len < 2 || i + len > cfg.len() {
            break;
        }
        let d = &cfg[i..i + len];
        match d[1] {
            4 if len >= 9 => {
                take(cur.take(), &mut found);
                if d[2] == interface && d[3] != 0 && d[5] == CLASS_AUDIO && d[6] == SUBCLASS_STREAMING {
                    cur = Some(Setting { alternate: d[3], endpoint: 0, max_packet: 0, channels: 0, rates: 0 });
                }
            }
            // A type I format: channels, subframe bytes, bits, then the
            // rates (0: a continuous range, low and high).
            CS_INTERFACE if len >= 8 && d[2] == AS_FORMAT_TYPE && d[3] == FORMAT_TYPE_I => {
                if let Some(s) = cur.as_mut().filter(|_| d[5] == 2 && d[6] == 16) {
                    s.channels = d[4];
                    let rate = |k: usize| d.get(8 + 3 * k..11 + 3 * k).map(|r| u32::from_le_bytes([r[0], r[1], r[2], 0]));
                    let mut rates = 0u32;
                    if d[7] == 0 {
                        if let (Some(lo), Some(hi)) = (rate(0), rate(1)) {
                            for (b, &r) in SOUND_RATES.iter().enumerate() {
                                if (lo..=hi).contains(&r) {
                                    rates |= 1 << b;
                                }
                            }
                        }
                    } else {
                        for k in 0..usize::from(d[7]) {
                            if let Some(b) = rate(k).and_then(|r| SOUND_RATES.iter().position(|&x| x == r)) {
                                rates |= 1 << b;
                            }
                        }
                    }
                    s.rates = rates;
                }
            }
            5 if len >= 7 => {
                if let Some(s) = cur.as_mut() {
                    if d[3] & 3 == 1 && d[2] & 0x80 == 0 && s.endpoint == 0 {
                        s.endpoint = d[2];
                        s.max_packet = usize::from(u16::from_le_bytes([d[4], d[5]]) & 0x7FF);
                    }
                }
            }
            _ => {}
        }
        i += len;
    }
    take(cur, &mut found);
    found
}

/// Queue packets until [`AHEAD`] are in flight, [`BATCH`] at a time: each
/// a millisecond of the core's PCM.
fn fill(c: &mut Card) {
    let core_card = c.card;
    let (dev, endpoint, max_packet) = (c.dev, c.endpoint, c.max_packet);
    let Some(run) = c.run.as_mut() else {
        return;
    };
    let mut data = [0u8; BATCH * USB_ISOCH_PACKET_MAX];
    while run.queued.wrapping_sub(run.done) + BATCH as u32 <= AHEAD {
        let mut lens = [0u16; BATCH];
        let mut at = 0;
        for len in lens.iter_mut() {
            run.carry += run.rate;
            let frames = run.carry / 1000;
            run.carry %= 1000;
            let bytes = (frames as usize * run.frame).min(max_packet) / run.frame * run.frame;
            let pcm = &mut data[at..at + bytes];
            let got = sound().playback_take(core_card, 0, pcm);
            pcm[got..].fill(0);
            *len = bytes as u16;
            at += bytes;
        }
        if host().isoch_queue(dev, endpoint, &data[..at], &lens) != 0 {
            return;
        }
        run.queued = run.queued.wrapping_add(BATCH as u32);
    }
}

/// Packets completed (the host's thread): queue the next ones.
unsafe extern "C" fn isoch_done(ctx: *mut c_void, packets: i32) {
    let Some(slot) = CARDS.get(ctx as usize) else {
        return;
    };
    let mut guard = slot.lock();
    let Some(c) = guard.as_mut() else {
        return;
    };
    let Some(run) = c.run.as_mut() else {
        return;
    };
    if packets < 0 {
        c.run = None;
        return;
    }
    run.done = run.done.wrapping_add(packets as u32);
    fill(c);
}

// ---- The core's calls --------------------------------------------------------

static CARD_OPS: SoundCardOps = SoundCardOps { version: SOUND_VERSION, start: card_start, stop: card_stop };

unsafe extern "C" fn card_start(ctx: usize, _stream: u32, params: *const SoundParams) -> i32 {
    // SAFETY: the core hands its parameters.
    let Some(p) = (unsafe { params.as_ref() }).copied() else {
        return -1;
    };
    let Some(slot) = CARDS.get(ctx) else {
        return -1;
    };
    let mut guard = slot.lock();
    let Some(c) = guard.as_mut() else {
        return -1;
    };
    if p.format != SOUND_FORMAT_S16 || p.channels != c.channels || c.run.is_some() {
        return -1;
    }
    let h = host();
    if h.set_interface(c.dev, c.interface, c.alternate) != 0 {
        return -1;
    }
    // SET_CUR of the endpoint's sampling frequency control (a speaker with
    // one rate may not have it: its answer does not matter).
    let rate = p.rate.to_le_bytes();
    let mut freq = [rate[0], rate[1], rate[2]];
    h.control(c.dev, 0x22, 0x01, 0x0100, u16::from(c.endpoint), &mut freq);
    if unsafe { (h.isoch_start)(c.dev, c.endpoint, isoch_done, ctx as *mut c_void) } != 0 {
        h.set_interface(c.dev, c.interface, 0);
        return -1;
    }
    c.run = Some(Run { rate: p.rate, frame: usize::from(c.channels) * 2, carry: 0, queued: 0, done: 0 });
    fill(c);
    if c.run.as_ref().is_some_and(|r| r.queued == 0) {
        // The controller does not drive isochronous transfers.
        c.run = None;
        h.isoch_stop(c.dev, c.endpoint);
        h.set_interface(c.dev, c.interface, 0);
        return -1;
    }
    0
}

unsafe extern "C" fn card_stop(ctx: usize, _stream: u32) {
    let Some(slot) = CARDS.get(ctx) else {
        return;
    };
    let mut guard = slot.lock();
    let Some(c) = guard.as_mut() else {
        return;
    };
    if c.run.take().is_some() {
        host().isoch_stop(c.dev, c.endpoint);
        host().set_interface(c.dev, c.interface, 0);
    }
}

// ---- The host's calls ----------------------------------------------------------

static DRIVER: UsbDriverOps = UsbDriverOps { name: StrRef { ptr: b"usb_audio".as_ptr(), len: 9 }, probe, disconnect };

/// The card's name for the core (`usb-audio 46f4:0002`).
struct Name([u8; 32], usize);

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

unsafe extern "C" fn probe(dev: *const UsbDeviceInfo, intf: *const UsbInterfaceInfo) -> i32 {
    let (dev, intf) = unsafe { (&*dev, &*intf) };
    if intf.class != CLASS_AUDIO {
        return -1;
    }
    // The control interface: the speaker's, nothing to drive there.
    if intf.subclass == SUBCLASS_CONTROL {
        return 0;
    }
    if intf.subclass != SUBCLASS_STREAMING {
        return -1;
    }
    let mut cfg = [0u8; 1024];
    let n = config_descriptor(dev.id, &mut cfg);
    let Some(setting) = find_setting(&cfg[..n], intf.number) else {
        return -1;
    };
    let Some(slot) = CARDS.iter().position(|s| s.lock().is_none()) else {
        return -1;
    };
    let info = SoundStreamInfo {
        direction: SOUND_PLAYBACK,
        channels_min: setting.channels,
        channels_max: setting.channels,
        formats: 1 << SOUND_FORMAT_S16,
        rates: setting.rates,
    };
    *CARDS[slot].lock() = Some(Card {
        dev: dev.id,
        interface: intf.number,
        alternate: setting.alternate,
        endpoint: setting.endpoint,
        max_packet: setting.max_packet,
        channels: setting.channels,
        card: -1,
        run: None,
    });
    let mut name = Name([0; 32], 0);
    let _ = write!(name, "usb-audio {:04x}:{:04x}", dev.vendor, dev.product);
    let card = sound().card_register(&CARD_OPS, slot, core::str::from_utf8(&name.0[..name.1]).unwrap_or("usb-audio"), &[info]);
    if card < 0 {
        *CARDS[slot].lock() = None;
        return -1;
    }
    if let Some(c) = CARDS[slot].lock().as_mut() {
        c.card = card;
    }
    host().interface_label(dev.id, intf.number, b"snd");
    status_ok(api(), "usb_audio: a speaker");
    0
}

unsafe extern "C" fn disconnect(dev: u32, interface: u8) {
    for slot in CARDS.iter() {
        let mut guard = slot.lock();
        if guard.as_ref().is_some_and(|c| c.dev == dev && c.interface == interface) {
            let c = guard.take().unwrap();
            drop(guard);
            sound().card_remove(c.card);
        }
    }
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
    let core = api.service_lookup(SOUND_SERVICE) as *mut SoundOps;
    // SAFETY: the sound module's table, loaded before this one.
    if core.is_null() || unsafe { (*core).version } != SOUND_VERSION {
        api.write_str("usb_audio: no sound core\n");
        return -3;
    }
    SOUND.store(core, Ordering::Release);
    let table = api.service_lookup(USB_SERVICE) as *mut UsbHostOps;
    // SAFETY: the usb module's table, loaded before this one.
    if table.is_null() || unsafe { (*table).version } != USB_HOST_VERSION {
        api.write_str("usb_audio: no usb host\n");
        return -3;
    }
    HOST.store(table, Ordering::Release);
    if host().driver_register(&DRIVER) != 0 {
        status_fail(api, "usb_audio: driver");
        return -5;
    }
    status_ok(api, "usb_audio");
    0
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn module_exit() {}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    let api = api();
    status_fail(api, "usb_audio: panic");
    loop {
        api.task_yield();
    }
}
