//! Event counters, timers and wait sets: `/dev/event/N`, `/dev/timer/N`
//! and `/dev/wait/N` (`docs/events.md`). An object comes from reading the
//! directory's `clone`, which allocates it and answers its number;
//! `N/data` opened is the fd a program holds (an event, timer or wait fd
//! in [`crate::task`], whose reads and writes come here), `N/ctl` takes
//! its commands as text and `N/status` describes it.
//!
//! An event counter is a `u64` a write adds to and a read takes: the whole
//! count (zeroed) or, as a semaphore, one at a time. A timer counts its
//! expirations until a read takes them; it is armed on the monotonic or
//! the wall clock, once or at an interval, and fires from the scheduler's
//! tick ([`tick`]) or the arming CPU's timer. Both are readable while they
//! hold something to take, which is what `poll` reports, so an event loop
//! waits for them like for a pipe. A wait set holds other fds' objects
//! and is readable while one of them is ready: a read gives their cookies
//! and readiness, so an event loop waits for many fds in one read. Linux's
//! `eventfd`, `timerfd` and `epoll` are these (libgloss `evfd.c`, the
//! Linux layer).

use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use spin::Mutex;

use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec::Vec;

use crate::task::FdEntry;

/// Linux's counter ceiling: a write that would pass it waits.
const COUNT_MAX: u64 = u64::MAX - 1;

/// The objects of one kind by id, as many as are open (each live one holds
/// an fd, so the fd limits bound them). A freed id is reused first. One
/// read from `clone` with its `data` never opened stays, as a `/net`
/// conversation does.
struct Table<T>(Mutex<Vec<Option<Box<T>>>>);

/// What every object has: the user whose process made it (only that
/// user's processes see it, as a `/net` conversation: `docs/security.md`)
/// and the fds referring to it (the object is freed once the last of them
/// closes).
struct Refs {
    owner: u32,
    fds: AtomicUsize,
}

impl Refs {
    fn new() -> Self {
        Self { owner: crate::sec::current_uid(), fds: AtomicUsize::new(0) }
    }

    /// Whether the current process's user may name the object.
    fn visible(&self) -> bool {
        self.owner == crate::sec::current_uid()
    }
}

impl<T> Table<T> {
    const fn new() -> Self {
        Self(Mutex::new(Vec::new()))
    }

    /// `item` on the heap in the lowest free slot: its id, or `None` with
    /// the heap full (a failed `clone`, not a kernel panic).
    fn alloc(&self, item: T) -> Option<usize> {
        let layout = core::alloc::Layout::new::<T>();
        // SAFETY: a fresh allocation of `T`'s layout, written before the
        // Box owns it.
        let boxed = unsafe {
            let raw = alloc::alloc::alloc(layout) as *mut T;
            if raw.is_null() {
                return None;
            }
            raw.write(item);
            Box::from_raw(raw)
        };
        let mut items = self.0.lock();
        if let Some(id) = items.iter().position(Option::is_none) {
            items[id] = Some(boxed);
            return Some(id);
        }
        items.try_reserve(1).ok()?;
        items.push(Some(boxed));
        Some(items.len() - 1)
    }

    fn get(&self, id: usize) -> Option<&'static T> {
        let items = self.0.lock();
        // SAFETY: an item is boxed (the table growing never moves it) and
        // only freed by replacing its slot with None once no fd refers to
        // it, and callers use it on behalf of an fd that does.
        let ptr = &**items.get(id)?.as_ref()? as *const T;
        drop(items);
        Some(unsafe { &*ptr })
    }

    fn free(&self, id: usize) {
        let mut items = self.0.lock();
        let Some(slot) = items.get_mut(id) else { return };
        let item = slot.take();
        drop(items);
        drop(item);
    }

    /// One past the highest id in use: the ids to look at.
    fn bound(&self) -> usize {
        self.0.lock().len()
    }
}

/// A `ctl` command without the whitespace around it (`echo` adds a newline).
fn trim(text: &[u8]) -> &[u8] {
    let ws = |b: &u8| matches!(b, b' ' | b'\t' | b'\n' | b'\r');
    let start = text.iter().position(|b| !ws(b)).unwrap_or(text.len());
    let end = text.iter().rposition(|b| !ws(b)).map_or(start, |i| i + 1);
    &text[start..end.max(start)]
}

/// An fd was opened on, duplicated to, or closed from the object: its
/// reference count.
fn refs_open(refs: &Refs) {
    refs.fds.fetch_add(1, Ordering::SeqCst);
}

fn refs_drop(refs: &Refs) -> bool {
    refs.fds.fetch_sub(1, Ordering::SeqCst) == 1
}

// ---- event counters ----------------------------------------------------

pub struct Event {
    count: Mutex<u64>,
    semaphore: AtomicBool,
    refs: Refs,
}

static EVENTS: Table<Event> = Table::new();

/// A new event counter (`clone`): its id.
pub fn event_alloc() -> Option<usize> {
    EVENTS.alloc(Event { count: Mutex::new(0), semaphore: AtomicBool::new(false), refs: Refs::new() })
}

/// Whether event `id` is there, for the current process's user.
pub fn event_exists(id: usize) -> bool {
    EVENTS.get(id).is_some_and(|e| e.refs.visible())
}

pub fn event_id_bound() -> usize {
    EVENTS.bound()
}

/// An fd opened on event `id` (`N/data`): false when there is no such event.
pub fn event_open(id: usize) -> bool {
    EVENTS.get(id).map(|e| refs_open(&e.refs)).is_some()
}

/// Reference count bump for `dup` / `fork` of an event fd.
pub fn event_ref(id: usize) {
    if let Some(e) = EVENTS.get(id) {
        e.refs.fds.fetch_add(1, Ordering::SeqCst);
    }
}

/// One fd referring to event `id` closed; the last frees it (true then).
pub fn event_drop(id: usize) -> bool {
    let last = EVENTS.get(id).is_some_and(|e| refs_drop(&e.refs));
    if last {
        EVENTS.free(id);
    }
    last
}

/// Take the count: the whole of it (zeroed), or one as a semaphore.
/// `None` while it is zero (a read waits), or for no such event.
pub fn event_read(id: usize) -> Option<u64> {
    let e = EVENTS.get(id)?;
    let taken = {
        let mut count = e.count.lock();
        if *count == 0 {
            return None;
        }
        if e.semaphore.load(Ordering::Relaxed) {
            *count -= 1;
            1
        } else {
            core::mem::take(&mut *count)
        }
    };
    // Room for a writer that waited at the ceiling, and news for pollers.
    crate::task::wake(crate::task::key_event(id));
    crate::task::wake_any();
    Some(taken)
}

/// What a write of `value` to event `id` did.
pub enum EventWrite {
    Done,
    /// The count would pass the ceiling: the write waits for a read.
    WouldBlock,
    /// `u64::MAX`, or no such event.
    Invalid,
}

pub fn event_write(id: usize, value: u64) -> EventWrite {
    let Some(e) = EVENTS.get(id) else {
        return EventWrite::Invalid;
    };
    if value == u64::MAX {
        return EventWrite::Invalid;
    }
    {
        let mut count = e.count.lock();
        match count.checked_add(value).filter(|&c| c <= COUNT_MAX) {
            Some(c) => *count = c,
            None => return EventWrite::WouldBlock,
        }
    }
    if value != 0 {
        crate::task::wake(crate::task::key_event(id));
        crate::task::wake_any();
    }
    EventWrite::Done
}

/// `(readable, writable)`: something to take, room for a write of 1.
pub fn event_poll(id: usize) -> (bool, bool) {
    match EVENTS.get(id) {
        Some(e) => {
            let count = *e.count.lock();
            (count > 0, count < COUNT_MAX)
        }
        None => (false, false),
    }
}

/// A command on event `id`'s `ctl`: `semaphore` (reads take one at a
/// time from now on), `counter` (the whole count), `add N` (what a write
/// of N to `data` does, for a shell or a kernel module; refused at the
/// ceiling). `None` refuses it.
pub fn event_ctl_write(id: usize, text: &[u8]) -> Option<usize> {
    let e = EVENTS.get(id)?;
    let line = core::str::from_utf8(trim(text)).ok()?;
    match line.split_once(' ') {
        None if line == "semaphore" => e.semaphore.store(true, Ordering::SeqCst),
        None if line == "counter" => e.semaphore.store(false, Ordering::SeqCst),
        Some(("add", n)) => {
            let value: u64 = n.trim().parse().ok()?;
            if !matches!(event_write(id, value), EventWrite::Done) {
                return None;
            }
        }
        _ => return None,
    }
    Some(text.len())
}

/// Event `id`'s `status`: `count N` and `mode counter|semaphore`.
pub fn event_status(id: usize) -> Option<Vec<u8>> {
    let e = EVENTS.get(id)?;
    let mode = if e.semaphore.load(Ordering::Relaxed) { "semaphore" } else { "counter" };
    Some(alloc::format!("count {}\nmode {mode}\n", *e.count.lock()).into_bytes())
}

// ---- timers ------------------------------------------------------------

pub struct Timer {
    state: Mutex<TimerState>,
    refs: Refs,
}

struct TimerState {
    /// The next expiry, monotonic ns; 0 while disarmed.
    next_ns: u64,
    /// Re-armed this much later at each expiry; 0 for one expiry.
    interval_ns: u64,
    /// Expiries no read took yet.
    expirations: u64,
}

impl TimerState {
    /// Count the expiries due at `now`, the timer re-armed past it or
    /// disarmed after its one; true when any was.
    fn fire(&mut self, now: u64) -> bool {
        if self.next_ns == 0 || self.next_ns > now {
            return false;
        }
        if self.interval_ns == 0 {
            self.expirations = self.expirations.saturating_add(1);
            self.next_ns = 0;
        } else {
            // Late by more than one interval: every missed expiry counts.
            let missed = (now - self.next_ns) / self.interval_ns + 1;
            self.expirations = self.expirations.saturating_add(missed);
            self.next_ns = self.next_ns.saturating_add(missed.saturating_mul(self.interval_ns));
        }
        true
    }
}

static TIMERS: Table<Timer> = Table::new();

/// A new, disarmed timer (`clone`): its id.
pub fn timer_alloc() -> Option<usize> {
    TIMERS.alloc(Timer {
        state: Mutex::new(TimerState { next_ns: 0, interval_ns: 0, expirations: 0 }),
        refs: Refs::new(),
    })
}

/// Whether timer `id` is there, for the current process's user.
pub fn timer_exists(id: usize) -> bool {
    TIMERS.get(id).is_some_and(|t| t.refs.visible())
}

pub fn timer_id_bound() -> usize {
    TIMERS.bound()
}

pub fn timer_open(id: usize) -> bool {
    TIMERS.get(id).map(|t| refs_open(&t.refs)).is_some()
}

pub fn timer_ref(id: usize) {
    if let Some(t) = TIMERS.get(id) {
        t.refs.fds.fetch_add(1, Ordering::SeqCst);
    }
}

/// One fd referring to timer `id` closed; the last frees it (true then).
pub fn timer_drop(id: usize) -> bool {
    let last = TIMERS.get(id).is_some_and(|t| refs_drop(&t.refs));
    if last {
        TIMERS.free(id);
    }
    last
}

/// Take the expirations counted so far (those due now included). `None`
/// while there are none (a read waits), or for no such timer.
pub fn timer_read(id: usize) -> Option<u64> {
    let t = TIMERS.get(id)?;
    let mut s = t.state.lock();
    s.fire(crate::time::monotonic_ns());
    if s.expirations == 0 {
        return None;
    }
    Some(core::mem::take(&mut s.expirations))
}

/// Readable: an expiry waits to be taken.
pub fn timer_poll(id: usize) -> bool {
    TIMERS.get(id).is_some_and(|t| {
        let mut s = t.state.lock();
        s.fire(crate::time::monotonic_ns());
        s.expirations > 0
    })
}

/// When timer `id` next expires (monotonic ns); 0 while disarmed. A
/// blocked reader sleeps until then.
pub fn timer_next_ns(id: usize) -> u64 {
    TIMERS.get(id).map_or(0, |t| t.state.lock().next_ns)
}

/// A command on timer `id`'s `ctl` (`docs/events.md`): `arm FIRST
/// [INTERVAL] [abs] [realtime]` (nanoseconds; FIRST relative, or a clock
/// value with `abs`; INTERVAL 0 or left out for one expiry; `realtime` on
/// the wall clock) and `disarm`. Arming forgets the expirations not read.
/// `None` refuses it (bad syntax, `realtime` before the RTC was read, or
/// no such timer).
pub fn timer_ctl_write(id: usize, text: &[u8]) -> Option<usize> {
    let t = TIMERS.get(id)?;
    let line = core::str::from_utf8(trim(text)).ok()?;
    let mut words = line.split_ascii_whitespace();
    let (next_ns, interval_ns) = match words.next()? {
        "disarm" => (0, 0),
        "arm" => {
            let first: u64 = words.next()?.parse().ok()?;
            let mut interval = 0;
            let (mut abs, mut realtime) = (false, false);
            for word in words {
                match word {
                    "abs" => abs = true,
                    "realtime" => realtime = true,
                    n => interval = n.parse().ok()?,
                }
            }
            let now = crate::time::monotonic_ns();
            let next = match (abs, realtime) {
                // A zero FIRST disarms, as Linux's zero `it_value` does.
                (false, _) if first == 0 => 0,
                (false, _) => now.saturating_add(first),
                (true, false) => first.max(1),
                (true, true) => crate::time::wall_to_monotonic(first as i64)?.max(1),
            };
            (next, if next == 0 { 0 } else { interval })
        }
        _ => return None,
    };
    {
        let mut s = t.state.lock();
        *s = TimerState { next_ns, interval_ns, expirations: 0 };
    }
    if next_ns != 0 {
        crate::task::deadline_soon(next_ns);
    }
    Some(text.len())
}

/// Timer `id`'s `status`: `armed LEFT INTERVAL` (nanoseconds until the
/// next expiry, and between expiries) or `disarmed`.
pub fn timer_status(id: usize) -> Option<Vec<u8>> {
    let t = TIMERS.get(id)?;
    let s = t.state.lock();
    Some(if s.next_ns == 0 {
        b"disarmed\n".to_vec()
    } else {
        let left = s.next_ns.saturating_sub(crate::time::monotonic_ns());
        alloc::format!("armed {left} {}\n", s.interval_ns).into_bytes()
    })
}

/// The scheduler's tick at `now`: count the expiries due, `fired(id)` for
/// each timer that had one (its readers and the pollers are woken), and
/// the earliest expiry still ahead (`u64::MAX`: none). Interrupt context:
/// a timer being armed on this CPU holds the table, and the next tick
/// sees its expiry.
pub fn tick(now: u64, mut fired: impl FnMut(usize)) -> u64 {
    let Some(timers) = TIMERS.0.try_lock() else {
        return now;
    };
    let mut next = u64::MAX;
    for (id, t) in timers.iter().enumerate() {
        let Some(t) = t else { continue };
        let Some(mut s) = t.state.try_lock() else {
            next = next.min(now);
            continue;
        };
        if s.fire(now) {
            fired(id);
        }
        if s.next_ns != 0 {
            next = next.min(s.next_ns);
        }
    }
    next
}

// ---- wait sets -----------------------------------------------------------

/// The readiness a member asks for and reports: `poll(2)`'s bits, which
/// are Linux's `EPOLL*` values too.
pub const WAIT_IN: u32 = 0x1;
pub const WAIT_OUT: u32 = 0x4;
pub const WAIT_ERR: u32 = 0x8;
pub const WAIT_HUP: u32 = 0x10;
/// `et`: a readiness is reported once, until the member was seen not
/// ready (an edge, near enough: Linux reports every new event).
const WAIT_ET: u32 = 1 << 31;
/// `oneshot`: reported once, then the member waits for a `mod`.
const WAIT_ONESHOT: u32 = 1 << 30;

/// Members a set holds at most (Linux has no limit; this bounds the heap
/// a process can take).
const WAIT_MEMBERS_MAX: usize = 1024;

/// A member of a wait set: an open object (what an fd refers to: a pipe
/// end, a socket's file, an event, ...), tagged with the fd it was added
/// by, in the process that added it. `mod` and `del` name it by that fd,
/// `add` refuses it twice. Closing the object's last fd removes it
/// ([`wait_forget`]), as on Linux.
struct Member {
    fd: usize,
    target: FdEntry,
    events: u32,
    cookie: u64,
    /// `et`: reported while ready, not seen not ready since.
    reported: bool,
    /// `oneshot`: reported, waiting for a `mod`.
    disabled: bool,
}

pub struct Wait {
    members: Mutex<Vec<Member>>,
    refs: Refs,
}

static WAITS: Table<Wait> = Table::new();

/// A new, empty wait set (`clone`): its id.
pub fn wait_alloc() -> Option<usize> {
    WAITS.alloc(Wait { members: Mutex::new(Vec::new()), refs: Refs::new() })
}

/// Whether wait set `id` is there, for the current process's user.
pub fn wait_exists(id: usize) -> bool {
    WAITS.get(id).is_some_and(|w| w.refs.visible())
}

pub fn wait_id_bound() -> usize {
    WAITS.bound()
}

pub fn wait_open(id: usize) -> bool {
    WAITS.get(id).map(|w| refs_open(&w.refs)).is_some()
}

pub fn wait_ref(id: usize) {
    if let Some(w) = WAITS.get(id) {
        w.refs.fds.fetch_add(1, Ordering::SeqCst);
    }
}

/// One fd referring to wait set `id` closed; the last frees it (true then).
pub fn wait_drop(id: usize) -> bool {
    let last = WAITS.get(id).is_some_and(|w| refs_drop(&w.refs));
    if last {
        WAITS.free(id);
    }
    last
}

/// A comma list of `in`, `out`, `et`, `oneshot` (or `none`) as bits.
fn wait_events(text: &str) -> Option<u32> {
    if text == "none" {
        return Some(0);
    }
    let mut bits = 0;
    for word in text.split(',') {
        bits |= match word {
            "in" => WAIT_IN,
            "out" => WAIT_OUT,
            "et" => WAIT_ET,
            "oneshot" => WAIT_ONESHOT,
            _ => return None,
        };
    }
    Some(bits)
}

fn wait_events_text(bits: u32) -> String {
    let words = [(WAIT_IN, "in"), (WAIT_OUT, "out"), (WAIT_ET, "et"), (WAIT_ONESHOT, "oneshot")];
    let text: Vec<&str> = words.iter().filter(|(bit, _)| bits & bit != 0).map(|(_, word)| *word).collect();
    if text.is_empty() { String::from("none") } else { text.join(",") }
}

/// Whether set `set` holds set `needle`, through at most `depth` more
/// sets: a set in itself, or a cycle of sets, would scan without end.
fn wait_holds(set: usize, needle: usize, depth: usize) -> bool {
    let Some(w) = WAITS.get(set) else { return false };
    let inner: Vec<usize> = w
        .members
        .lock()
        .iter()
        .filter_map(|m| if let FdEntry::Wait(id) = m.target { Some(id) } else { None })
        .collect();
    inner.iter().any(|&id| id == needle || (depth > 0 && wait_holds(id, needle, depth - 1)))
}

/// A command on wait set `id`'s `ctl` (`docs/events.md`): `add FD EVENTS
/// COOKIE` (FD an fd of the writing process, EVENTS a comma list of `in`,
/// `out`, `et`, `oneshot`, or `none`; COOKIE what a read reports for it),
/// `mod FD EVENTS COOKIE` (new events and cookie, re-armed), `del FD`.
/// Refused (`None`): a bad line, an fd that is not open, `add` of a member
/// the set has, of the set itself or of a set holding this one; `mod` or
/// `del` of what is no member.
pub fn wait_ctl_write(id: usize, text: &[u8]) -> Option<usize> {
    let w = WAITS.get(id)?;
    let line = core::str::from_utf8(trim(text)).ok()?;
    let mut words = line.split_ascii_whitespace();
    let verb = words.next()?;
    let fd: usize = words.next()?.parse().ok()?;
    let target = crate::task::fd_entry(fd)?;
    match verb {
        "add" | "mod" => {
            let events = wait_events(words.next()?)?;
            let cookie: u64 = words.next()?.parse().ok()?;
            if words.next().is_some() {
                return None;
            }
            let member = Member { fd, target, events, cookie, reported: false, disabled: false };
            if verb == "add" {
                if let FdEntry::Wait(other) = target {
                    if other == id || wait_holds(other, id, 4) {
                        return None;
                    }
                }
                let mut members = w.members.lock();
                if members.iter().any(|m| m.fd == fd && m.target == target)
                    || members.len() >= WAIT_MEMBERS_MAX
                    || members.try_reserve(1).is_err()
                {
                    return None;
                }
                members.push(member);
            } else {
                let mut members = w.members.lock();
                let m = members.iter_mut().find(|m| m.fd == fd && m.target == target)?;
                *m = member;
            }
        }
        "del" => {
            if words.next().is_some() {
                return None;
            }
            let mut members = w.members.lock();
            let i = members.iter().position(|m| m.fd == fd && m.target == target)?;
            members.remove(i);
        }
        _ => return None,
    }
    // A member added or re-armed may be ready: the set's readers look.
    crate::task::wake_any();
    Some(text.len())
}

/// The ready members of set `id`, up to `max` of them, as `(cookie,
/// readiness)`; with `take`, an `et` member's readiness counts as reported
/// and a `oneshot` one is disabled (what a read does; `poll` only looks).
/// The second result: whether a member is the console tty, which a waiter
/// re-checks when the keyboard is polled.
pub fn wait_scan(id: usize, max: usize, take: bool) -> (Vec<(u64, u32)>, bool) {
    let mut out = Vec::new();
    let mut tty = false;
    let Some(w) = WAITS.get(id) else { return (out, false) };
    // The members' readiness is asked outside the lock: a member that is
    // a set itself scans its own.
    let snapshot: Vec<(usize, FdEntry, u32)> =
        w.members.lock().iter().filter(|m| !m.disabled).map(|m| (m.fd, m.target, m.events)).collect();
    let mut ready = Vec::new();
    for (fd, target, events) in snapshot {
        let (bits, is_tty) = crate::task::entry_poll(target);
        tty |= is_tty && events & WAIT_IN != 0;
        let hit = u32::from(bits) & (events & (WAIT_IN | WAIT_OUT) | WAIT_ERR | WAIT_HUP);
        ready.push((fd, target, hit));
    }
    let mut members = w.members.lock();
    for (fd, target, hit) in ready {
        let Some(m) = members.iter_mut().find(|m| m.fd == fd && m.target == target) else { continue };
        if hit == 0 {
            if take {
                m.reported = false;
            }
            continue;
        }
        if m.events & WAIT_ET != 0 && m.reported {
            continue;
        }
        if out.len() >= max {
            // What the reader has no room for stays as it is.
            break;
        }
        if take {
            if m.events & WAIT_ET != 0 {
                m.reported = true;
            }
            if m.events & WAIT_ONESHOT != 0 {
                m.disabled = true;
            }
        }
        out.push((m.cookie, hit));
    }
    (out, tty)
}

/// Whether set `id` has a ready member (what `poll` reports for its fd),
/// and whether one is the console tty.
pub fn wait_ready(id: usize) -> (bool, bool) {
    let (ready, tty) = wait_scan(id, 1, false);
    (!ready.is_empty(), tty)
}

/// The object `target`'s last fd closed: no set holds it any more.
pub fn wait_forget(target: FdEntry) {
    for id in 0..WAITS.bound() {
        if let Some(w) = WAITS.get(id) {
            w.members.lock().retain(|m| m.target != target);
        }
    }
}

/// Wait set `id`'s `status`: `members N`, then one line per member, `FD
/// EVENTS COOKIE`, with ` disabled` after a `oneshot` one that fired.
pub fn wait_status(id: usize) -> Option<Vec<u8>> {
    let w = WAITS.get(id)?;
    let members = w.members.lock();
    let mut text = alloc::format!("members {}\n", members.len());
    for m in members.iter() {
        let disabled = if m.disabled { " disabled" } else { "" };
        text += &alloc::format!("{} {} {}{disabled}\n", m.fd, wait_events_text(m.events), m.cookie);
    }
    Some(text.into_bytes())
}
