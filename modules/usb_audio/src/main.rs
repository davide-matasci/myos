//! USB Audio playback, Class 1 (a full-speed speaker, QEMU's `usb-audio`)
//! and Class 2 (high-speed DACs and interfaces, and full-speed ones): the
//! audio streaming interface's isochronous OUT endpoint, as a sound card
//! of the sound core (`docs/sound.md`).
//!
//! The driver takes a device's audio control interface (nothing to do
//! there) and its streaming interface, whose alternate setting with an
//! isochronous OUT endpoint and a type I PCM format of 16, 24 or 32-bit
//! slots it plays through (a 16-bit one first): the card's one playback
//! stream, its channels and the rates the device offers. UAC1 lists the
//! rates in the format; UAC2 has the stream's clock source answer a RANGE
//! request (the clock the terminal the interface links to names, through
//! a selector's current input or a multiplier). A started stream selects
//! that setting (the host gives the endpoint its bandwidth), sets the
//! sampling frequency (UAC1: the endpoint's control; UAC2: the clock's)
//! and keeps about [`AHEAD_MS`] of packets queued, one per service
//! interval of the endpoint (a millisecond at full speed, its microframes
//! at high speed): as packets complete, the host's thread calls back and
//! the driver queues the next ones from the core (silence for what it
//! does not have), widening the core's 16-bit samples to the slot.
//! Stopping goes back to setting 0. An asynchronous endpoint's feedback
//! (an isochronous IN one, issue #610) is not read: the packets carry the
//! nominal rate.

#![no_std]
#![no_main]
#![deny(unsafe_op_in_unsafe_fn)]

use core::ffi::c_void;
use core::fmt::Write;
use core::sync::atomic::{AtomicPtr, Ordering};

use myos_abi::{
    status_fail, status_ok, ApiCell, KernelApi, SleepLock, SoundCardOps, SoundOps, SoundParams, SoundStreamInfo,
    StrRef, UsbDeviceInfo, UsbDriverOps, UsbHostOps, UsbInterfaceInfo, ABI_VERSION, SOUND_FORMAT_S16, SOUND_PLAYBACK,
    SOUND_RATES, SOUND_SERVICE, SOUND_VERSION, USB_HOST_VERSION, USB_ISOCH_PACKET_MAX, USB_SERVICE, USB_SPEED_HIGH,
};

const MAX_CARDS: usize = 4;
const CLASS_AUDIO: u8 = 1;
const SUBCLASS_CONTROL: u8 = 1;
const SUBCLASS_STREAMING: u8 = 2;
/// `bInterfaceProtocol` of a UAC2 interface.
const PROTOCOL_UAC2: u8 = 0x20;
/// Class-specific interface descriptor, and the subtypes this driver
/// reads: a streaming interface's general one and its format, a control
/// interface's input terminal and clock entities (UAC2).
const CS_INTERFACE: u8 = 0x24;
const AS_GENERAL: u8 = 1;
const AS_FORMAT_TYPE: u8 = 2;
const AC_INPUT_TERMINAL: u8 = 2;
const AC_CLOCK_SOURCE: u8 = 0x0A;
const AC_CLOCK_SELECTOR: u8 = 0x0B;
const AC_CLOCK_MULTIPLIER: u8 = 0x0C;
const FORMAT_TYPE_I: u8 = 1;
/// UAC2 requests (class, interface): CUR and RANGE of a clock's controls.
const UAC2_CUR: u8 = 1;
const UAC2_RANGE: u8 = 2;
/// The control selectors: a clock source's sampling frequency, a clock
/// selector's input.
const CS_SAM_FREQ_CONTROL: u16 = 1;
const CX_CLOCK_SELECTOR_CONTROL: u16 = 1;
/// How much is kept queued, a third of it queued at a time; never more
/// than this many packets in flight.
const AHEAD_MS: u32 = 24;
const AHEAD_MAX: u32 = 48;
/// The bytes one batch carries at most (a stack buffer).
const BATCH_BYTES: usize = 8192;
/// UAC2 clock entities the driver follows, and RANGE subranges it reads.
const MAX_ENTITIES: usize = 16;
const MAX_SUBRANGES: usize = 16;

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

/// A running stream: its rate, a frame's bytes on the bus, the fraction
/// of a frame each packet carries over (44.1 kHz at a packet a
/// millisecond: 44 or 45 frames), the packets queued and completed so
/// far, how many to keep queued and how many go at once.
struct Run {
    rate: u32,
    frame: usize,
    carry: u32,
    queued: u32,
    done: u32,
    ahead: u32,
    batch: usize,
}

struct Card {
    dev: u32,
    interface: u8,
    alternate: u8,
    endpoint: u8,
    /// A packet's bytes at most.
    max_packet: usize,
    channels: u8,
    /// A sample's bytes on the bus (2, 3 or 4: the core's 16 bits in the
    /// top ones).
    subslot: u8,
    /// Service intervals a second (packets).
    pps: u32,
    /// UAC2: the clock source and the control interface it is reached
    /// through.
    clock: Option<(u8, u8)>,
    /// The core's number for the card.
    card: i32,
    run: Option<Run>,
}

static CARDS: [SleepLock<Option<Card>>; MAX_CARDS] = [const { SleepLock::new(&API, None) }; MAX_CARDS];

/// What a streaming interface offers: the alternate setting, its
/// isochronous OUT endpoint (address, `wMaxPacketSize`, `bInterval`),
/// the channels and slot of its PCM format, and its rates (`SOUND_RATES`
/// bits: UAC1's from the format; UAC2's from the clock, `terminal` the
/// input terminal it links to).
#[derive(Clone, Copy)]
struct Setting {
    alternate: u8,
    endpoint: u8,
    max_packet: u16,
    interval: u8,
    channels: u8,
    subslot: u8,
    bits: u8,
    rates: u32,
    terminal: u8,
}

impl Setting {
    fn usable(&self, uac2: bool) -> bool {
        self.endpoint != 0
            && self.channels != 0
            && matches!(self.subslot, 2..=4)
            && self.bits >= 16
            && u32::from(self.bits) <= u32::from(self.subslot) * 8
            && (uac2 || self.rates != 0)
    }
}

/// A UAC2 control interface's clock topology: input terminals (their
/// clock), clock sources, selectors (their inputs) and multipliers (their
/// source), by id.
#[derive(Clone, Copy, Default)]
struct Clocks {
    terminals: [(u8, u8); MAX_ENTITIES],
    sources: [u8; MAX_ENTITIES],
    selectors: [(u8, [u8; 4]); MAX_ENTITIES],
    multipliers: [(u8, u8); MAX_ENTITIES],
}

/// What [`parse`] finds for a streaming interface: whether it is UAC2,
/// the control interface before it, that one's clocks, and the setting to
/// play through.
struct Found {
    uac2: bool,
    control: u8,
    clocks: Clocks,
    setting: Setting,
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

/// The `SOUND_RATES` bits of the rates from `lo` to `hi` (every `res` Hz
/// from `lo`, or any).
fn rates_between(lo: u32, hi: u32, res: u32) -> u32 {
    let mut rates = 0u32;
    for (b, &r) in SOUND_RATES.iter().enumerate() {
        if (lo..=hi).contains(&r) && (res == 0 || (r - lo) % res == 0) {
            rates |= 1 << b;
        }
    }
    rates
}

/// The settings of streaming interface `interface` in `cfg` this driver
/// plays through: the first with 16-bit slots, else the first.
fn parse(cfg: &[u8], interface: u8) -> Option<Found> {
    let mut i = 0usize;
    // The interface the descriptors belong to: (number, subclass, protocol).
    let mut within = (0xFFu8, 0u8, 0u8);
    let mut control = 0u8;
    let mut clocks = Clocks::default();
    let mut ours: Option<(u8, u8)> = None;
    let mut cur: Option<Setting> = None;
    let mut best: Option<Setting> = None;
    let mut uac2 = false;
    let take = |s: Option<Setting>, uac2: bool, best: &mut Option<Setting>| {
        if let Some(s) = s.filter(|s| s.usable(uac2)) {
            if best.is_none_or(|b| b.subslot != 2 && s.subslot == 2) {
                *best = Some(s);
            }
        }
    };
    let add = |list: &mut [(u8, u8)], e: (u8, u8)| {
        if let Some(x) = list.iter_mut().find(|x| x.0 == 0) {
            *x = e;
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
                take(cur.take(), uac2, &mut best);
                within = if d[5] == CLASS_AUDIO { (d[2], d[6], d[7]) } else { (0xFF, 0, 0) };
                if d[5] == CLASS_AUDIO && d[6] == SUBCLASS_CONTROL && ours.is_none() {
                    control = d[2];
                    clocks = Clocks::default();
                }
                if d[2] == interface && d[5] == CLASS_AUDIO && d[6] == SUBCLASS_STREAMING {
                    ours = Some((control, d[7]));
                    uac2 = d[7] == PROTOCOL_UAC2;
                    if d[3] != 0 {
                        cur = Some(Setting { alternate: d[3], endpoint: 0, max_packet: 0, interval: 1, channels: 0, subslot: 0, bits: 0, rates: 0, terminal: 0 });
                    }
                }
            }
            // A UAC2 control interface's terminals and clocks.
            CS_INTERFACE if within.1 == SUBCLASS_CONTROL && within.2 == PROTOCOL_UAC2 && len >= 4 => match d[2] {
                AC_INPUT_TERMINAL if len >= 8 => add(&mut clocks.terminals, (d[3], d[7])),
                AC_CLOCK_SOURCE => {
                    if let Some(x) = clocks.sources.iter_mut().find(|x| **x == 0) {
                        *x = d[3];
                    }
                }
                AC_CLOCK_SELECTOR if len >= 5 => {
                    let n = usize::from(d[4]).min(4).min(len.saturating_sub(5));
                    let mut ins = [0u8; 4];
                    ins[..n].copy_from_slice(&d[5..5 + n]);
                    if let Some(x) = clocks.selectors.iter_mut().find(|x| x.0 == 0) {
                        *x = (d[3], ins);
                    }
                }
                AC_CLOCK_MULTIPLIER if len >= 5 => add(&mut clocks.multipliers, (d[3], d[4])),
                _ => {}
            },
            CS_INTERFACE if cur.is_some() => {
                let s = cur.as_mut().unwrap();
                match (uac2, d[2]) {
                    // UAC2: the terminal, a type I PCM format and the
                    // channels; then the slot and the bits.
                    (true, AS_GENERAL) if len >= 11 => {
                        let formats = u32::from_le_bytes([d[6], d[7], d[8], d[9]]);
                        if d[5] == FORMAT_TYPE_I && formats & 1 != 0 {
                            s.terminal = d[3];
                            s.channels = d[10];
                        }
                    }
                    (true, AS_FORMAT_TYPE) if len >= 6 && d[3] == FORMAT_TYPE_I => {
                        s.subslot = d[4];
                        s.bits = d[5];
                    }
                    // UAC1's type I format: channels, subframe bytes, bits,
                    // then the rates (0: a continuous range, low and high).
                    (false, AS_FORMAT_TYPE) if len >= 8 && d[3] == FORMAT_TYPE_I => {
                        s.channels = d[4];
                        s.subslot = d[5];
                        s.bits = d[6];
                        let rate = |k: usize| d.get(8 + 3 * k..11 + 3 * k).map(|r| u32::from_le_bytes([r[0], r[1], r[2], 0]));
                        let mut rates = 0u32;
                        if d[7] == 0 {
                            if let (Some(lo), Some(hi)) = (rate(0), rate(1)) {
                                rates = rates_between(lo, hi, 0);
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
                    _ => {}
                }
            }
            5 if len >= 7 => {
                if let Some(s) = cur.as_mut() {
                    if d[3] & 3 == 1 && d[2] & 0x80 == 0 && s.endpoint == 0 {
                        s.endpoint = d[2];
                        s.max_packet = u16::from_le_bytes([d[4], d[5]]);
                        s.interval = d[6];
                    }
                }
            }
            _ => {}
        }
        i += len;
    }
    take(cur, uac2, &mut best);
    let (control, _) = ours?;
    Some(Found { uac2, control, clocks, setting: best? })
}

/// UAC2: the clock source behind `terminal`'s clock: through a
/// selector's current input (its first when it does not say) or a
/// multiplier's source.
fn clock_source(dev: u32, control: u8, clocks: &Clocks, terminal: u8) -> Option<u8> {
    let mut id = clocks.terminals.iter().find(|t| t.0 == terminal && t.0 != 0)?.1;
    for _ in 0..MAX_ENTITIES {
        if clocks.sources.contains(&id) {
            return Some(id);
        }
        if let Some(&(_, ins)) = clocks.selectors.iter().find(|s| s.0 == id && id != 0) {
            let mut pin = [0u8; 1];
            let n = host().control(dev, 0xA1, UAC2_CUR, CX_CLOCK_SELECTOR_CONTROL << 8, u16::from(id) << 8 | u16::from(control), &mut pin);
            let pin = if n == 1 && (1..=4).contains(&pin[0]) { usize::from(pin[0]) - 1 } else { 0 };
            id = ins[pin];
        } else if let Some(&(_, src)) = clocks.multipliers.iter().find(|m| m.0 == id && id != 0) {
            id = src;
        } else {
            return None;
        }
    }
    None
}

/// UAC2: the `SOUND_RATES` bits of the rates clock source `clock` can run
/// at (its sampling frequency's RANGE: subranges of minimum, maximum and
/// resolution).
fn clock_rates(dev: u32, control: u8, clock: u8) -> u32 {
    let index = u16::from(clock) << 8 | u16::from(control);
    let mut head = [0u8; 2];
    if host().control(dev, 0xA1, UAC2_RANGE, CS_SAM_FREQ_CONTROL << 8, index, &mut head) != 2 {
        return 0;
    }
    let n = usize::from(u16::from_le_bytes(head)).min(MAX_SUBRANGES);
    let mut buf = [0u8; 2 + 12 * MAX_SUBRANGES];
    let len = 2 + 12 * n;
    let got = host().control(dev, 0xA1, UAC2_RANGE, CS_SAM_FREQ_CONTROL << 8, index, &mut buf[..len]);
    if got < len as i32 {
        return 0;
    }
    let mut rates = 0;
    for k in 0..n {
        let at = 2 + 12 * k;
        let word = |o: usize| u32::from_le_bytes([buf[at + o], buf[at + o + 1], buf[at + o + 2], buf[at + o + 3]]);
        rates |= rates_between(word(0), word(4), word(8));
    }
    rates
}

/// Queue packets until the run's `ahead` are in flight, `batch` at a
/// time: each a service interval of the core's PCM, its 16-bit samples
/// widened to the slot.
fn fill(c: &mut Card) {
    let core_card = c.card;
    let (dev, endpoint, max_packet, pps) = (c.dev, c.endpoint, c.max_packet, c.pps);
    let (channels, subslot) = (usize::from(c.channels), usize::from(c.subslot));
    let Some(run) = c.run.as_mut() else {
        return;
    };
    let mut data = [0u8; BATCH_BYTES];
    let mut pcm = [0u8; USB_ISOCH_PACKET_MAX];
    while run.queued.wrapping_sub(run.done) + run.batch as u32 <= run.ahead {
        let mut lens = [0u16; AHEAD_MAX as usize];
        let mut at = 0;
        for len in lens[..run.batch].iter_mut() {
            run.carry += run.rate;
            let frames = run.carry / pps;
            run.carry %= pps;
            let frames = (frames as usize).min(max_packet / run.frame);
            let bytes = frames * run.frame;
            let samples = frames * channels;
            let src = &mut pcm[..samples * 2];
            let got = sound().playback_take(core_card, 0, src);
            src[got..].fill(0);
            let out = &mut data[at..at + bytes];
            if subslot == 2 {
                out.copy_from_slice(src);
            } else {
                // Little endian, the sample in the top bytes of its slot.
                for (k, s) in src.chunks_exact(2).enumerate() {
                    let o = &mut out[k * subslot..(k + 1) * subslot];
                    o.fill(0);
                    o[subslot - 2..].copy_from_slice(s);
                }
            }
            *len = bytes as u16;
            at += bytes;
        }
        if host().isoch_queue(dev, endpoint, &data[..at], &lens[..run.batch]) != 0 {
            return;
        }
        run.queued = run.queued.wrapping_add(run.batch as u32);
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
    // Never more done than queued (the host counts what left the ring).
    if run.queued.wrapping_sub(run.done) > run.ahead {
        run.done = run.queued;
    }
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
    let rate = p.rate.to_le_bytes();
    match c.clock {
        // UAC2: the clock source's CUR sampling frequency.
        Some((clock, control)) => {
            let mut freq = rate;
            h.control(c.dev, 0x21, UAC2_CUR, CS_SAM_FREQ_CONTROL << 8, u16::from(clock) << 8 | u16::from(control), &mut freq);
        }
        // UAC1: SET_CUR of the endpoint's sampling frequency control (a
        // speaker with one rate may not have it: its answer does not
        // matter).
        None => {
            let mut freq = [rate[0], rate[1], rate[2]];
            h.control(c.dev, 0x22, 0x01, 0x0100, u16::from(c.endpoint), &mut freq);
        }
    }
    if unsafe { (h.isoch_start)(c.dev, c.endpoint, isoch_done, ctx as *mut c_void) } != 0 {
        h.set_interface(c.dev, c.interface, 0);
        return -1;
    }
    // A third of what is kept queued at a time, whole frames of a
    // high-speed endpoint's microframe packets, within the batch buffer.
    let frame = usize::from(c.channels) * usize::from(c.subslot);
    let ahead = (c.pps * AHEAD_MS / 1000).clamp(3, AHEAD_MAX);
    let per_frame = (c.pps / 1000).max(1) as usize;
    let mut batch = (ahead as usize / 3).min(BATCH_BYTES / c.max_packet.max(1)).max(1);
    if batch >= per_frame {
        batch -= batch % per_frame;
    }
    c.run = Some(Run { rate: p.rate, frame, carry: 0, queued: 0, done: 0, ahead, batch });
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
    // A UAC2 device's, with its clocks and several settings, runs longer
    // than a speaker's.
    let mut cfg = [0u8; 2048];
    let n = config_descriptor(dev.id, &mut cfg);
    let Some(found) = parse(&cfg[..n], intf.number) else {
        return -1;
    };
    let mut setting = found.setting;
    let clock = if found.uac2 {
        let Some(clock) = clock_source(dev.id, found.control, &found.clocks, setting.terminal) else {
            return -1;
        };
        setting.rates = clock_rates(dev.id, found.control, clock);
        if setting.rates == 0 {
            return -1;
        }
        Some((clock, found.control))
    } else {
        None
    };
    // Packets a second: a service interval of 2^(bInterval - 1) frames at
    // full speed, microframes at high speed; up to three transactions of
    // `wMaxPacketSize` a microframe there.
    let exp = u32::from(setting.interval.clamp(1, 16)) - 1;
    let high = dev.speed >= USB_SPEED_HIGH;
    let pps = ((if high { 8000 } else { 1000 }) >> exp).max(1);
    let mps = usize::from(setting.max_packet & 0x7FF);
    let transactions = if high { usize::from((setting.max_packet >> 11) & 3) + 1 } else { 1 };
    let max_packet = (mps * transactions).min(USB_ISOCH_PACKET_MAX);
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
        max_packet,
        channels: setting.channels,
        subslot: setting.subslot,
        pps,
        clock,
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
    // `/proc/usb` names it; a console line here would land in the middle
    // of whatever the console prints (the probe runs on the host's thread).
    host().interface_label(dev.id, intf.number, b"snd");
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
fn panic(info: &core::panic::PanicInfo) -> ! {
    let api = api();
    let mut line = Name([0; 32], 0);
    let _ = write!(line, "usb_audio: panic at {}", info.location().map_or(0, |l| l.line()));
    status_fail(api, core::str::from_utf8(&line.0[..line.1]).unwrap_or("usb_audio: panic"));
    loop {
        api.task_yield();
    }
}
