//! The driver (what the `wifi` daemon runs on an AX201) against a model of
//! the device (`ax201/`): its bring-up with MSI and MSI-X, a scan, joining
//! an access point and leaving it, with the model's checks on everything
//! it does.

mod ax201;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use iwx::dev::{Dev, Device, Dma, Peer};
use iwx::fw::Firmware;
use iwx::hw::Bus;
use iwx::msg::{self, Channel};
use iwx::sta::Beacon;

use ax201::device::{Ax201, Irq, IrqMode, MAC};

/// The interrupt as the driver waits for it: the rises counted.
#[derive(Default)]
struct Rises {
    count: Mutex<u64>,
    cv: Condvar,
}

struct Sink {
    rises: Arc<Rises>,
    mode: IrqMode,
    level: bool,
}

impl Irq for Sink {
    fn set(&mut self, level: bool) {
        if level && !self.level {
            *self.rises.count.lock().unwrap() += 1;
            self.rises.cv.notify_all();
        }
        self.level = level;
    }

    fn mode(&self) -> IrqMode {
        self.mode
    }
}

/// The driver's platform: the model's registers, heap buffers at made-up
/// bus addresses mapped into the model, its interrupt.
struct Host {
    dev: Arc<Mutex<Ax201>>,
    rises: Arc<Rises>,
    next: u64,
    start: Instant,
}

impl Bus for Host {
    fn read32(&mut self, off: u32) -> u32 {
        self.dev.lock().unwrap().read(off, 4) as u32
    }
    fn write32(&mut self, off: u32, val: u32) {
        self.dev.lock().unwrap().write(off, 4, u64::from(val));
    }
    fn write8(&mut self, off: u32, val: u8) {
        self.dev.lock().unwrap().write(off, 1, u64::from(val));
    }
    fn delay_us(&mut self, us: u32) {
        std::thread::sleep(Duration::from_micros(u64::from(us)));
    }
}

impl Device for Host {
    fn dma(&mut self, len: usize) -> Option<Dma> {
        let mem: &'static mut [u8] = Box::leak(vec![0u8; len + 4096].into_boxed_slice());
        let ptr = mem.as_mut_ptr().wrapping_add(mem.as_ptr().align_offset(4096));
        let phys = self.next;
        self.next += (len as u64).next_multiple_of(0x10_0000);
        // SAFETY: leaked, so it lives on; shared with the model as the
        // device's DMA memory is.
        unsafe { self.dev.lock().unwrap().mem.map(phys, len as u64, ptr, true) };
        Some(unsafe { Dma::new(ptr, len, phys) })
    }
    fn wait_irq(&mut self, timeout_us: u64) -> bool {
        let c = self.rises.count.lock().unwrap();
        let (mut c, _) = self.rises.cv.wait_timeout_while(c, Duration::from_micros(timeout_us), |c| *c == 0).unwrap();
        let fired = *c > 0;
        *c = 0;
        fired
    }
    fn now_us(&mut self) -> u64 {
        self.start.elapsed().as_micros() as u64
    }
}

/// The model with the firmware's time running, and the driver's host.
struct Rig {
    dev: Arc<Mutex<Ax201>>,
    stop: Arc<AtomicBool>,
}

impl Drop for Rig {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

fn rig(msix: bool) -> (Rig, Host) {
    let rises = Arc::new(Rises::default());
    let mode = if msix { IrqMode::Msix } else { IrqMode::Msi };
    let dev = Arc::new(Mutex::new(Ax201::new(Box::new(Sink { rises: rises.clone(), mode, level: false }))));
    let stop = Arc::new(AtomicBool::new(false));
    let (d, s) = (dev.clone(), stop.clone());
    std::thread::spawn(move || {
        while !s.load(Ordering::Relaxed) {
            let next = d.lock().unwrap().tick();
            std::thread::sleep(next.clamp(Duration::from_millis(1), Duration::from_millis(20)));
        }
    });
    let host = Host { dev: dev.clone(), rises, next: 0x1_0000_0000, start: Instant::now() };
    (Rig { dev, stop }, host)
}

fn failed(r: &Rig) -> Option<String> {
    r.dev.lock().unwrap().failed.clone()
}

fn firmware() -> &'static [u8] {
    Box::leak(iwx::sim::firmware().into_boxed_slice())
}

#[test]
fn brought_up_with_msi_and_msix() {
    let file = firmware();
    let fw = Firmware::parse(file).unwrap();
    for msix in [false, true] {
        let (r, host) = rig(msix);
        let mut dev = Dev::new(host, msix).unwrap();
        dev.load(&fw).unwrap_or_else(|e| panic!("load (MSI-X {msix}): {e:?}, {:?}", failed(&r)));
        let info = dev.init(&fw).unwrap_or_else(|e| panic!("init (MSI-X {msix}): {e:?}, {:?}", failed(&r)));
        assert_eq!(failed(&r), None);
        assert_eq!((info.mac, info.alpha2, info.tx_chains, info.rx_chains), (MAC, *b"ZZ", 3, 3));
        // 2.4 GHz 1-13 active; 5 GHz UNII-1 and -3 active, UNII-2A and
        // -2C passive (radar).
        assert_eq!(info.channels.len(), 13 + 4 + 4 + 12 + 5);
        assert!(info.channels.iter().any(|c| c.number == 52 && c.ghz5 && c.passive));
    }
}

#[test]
fn not_the_simulated_firmware() {
    // The right file with one byte of a section changed: the self load
    // notices, and the driver's wait for ALIVE ends in the firmware's
    // error.
    let mut file = iwx::sim::firmware();
    let at = file.len() - 50;
    file[at] ^= 0xff;
    let file: &'static [u8] = Box::leak(file.into_boxed_slice());
    let fw = Firmware::parse(file).unwrap();
    let (r, host) = rig(false);
    let mut dev = Dev::new(host, false).unwrap();
    assert!(dev.load(&fw).is_err());
    assert!(failed(&r).unwrap().contains("not the simulated firmware"), "{:?}", failed(&r));
}

#[test]
fn a_register_it_does_not_have() {
    let (r, mut host) = rig(false);
    host.write32(0x1234, 1);
    assert!(failed(&r).unwrap().contains("register 0x1234"));
}

/// Up, scanned: the frames heard and the scan's end.
fn scan(dev: &mut Dev<Host>, fw: &Firmware, info: &iwx::dev::Info) -> Vec<msg::Mpdu> {
    dev.scan(fw, info).unwrap();
    let mut heard = Vec::new();
    loop {
        let n = dev.next(15_000_000).unwrap().expect("the scan ends");
        match n.code {
            msg::REPLY_RX_MPDU => heard.push(msg::mpdu(&n.data).unwrap()),
            msg::SCAN_COMPLETE_UMAC => return heard,
            _ => {}
        }
    }
}

#[test]
fn scan_join_send_leave() {
    let file = firmware();
    let fw = Firmware::parse(file).unwrap();
    let (r, host) = rig(true);
    let mut dev = Dev::new(host, true).unwrap();
    dev.load(&fw).unwrap();
    let info = dev.init(&fw).unwrap();
    let heard = scan(&mut dev, &fw, &info);
    assert_eq!(failed(&r), None);
    // The three access points' beacons, each on its channel at its signal.
    let mut aps: Vec<(u8, u8, i16)> = heard.iter().filter(|m| m.frame[0] == 0x80).map(|m| (m.frame[21], m.channel, m.dbm)).collect();
    aps.sort();
    aps.dedup();
    assert_eq!(aps, [(0x01, 1, -40), (0x06, 6, -55), (0x0b, 11, -82)]);

    // Join myos-open, authenticate (open system), associate, leave.
    let bssid = [0x02, 0x00, 0x5e, 0x00, 0x01, 0x01];
    let channel = Channel { number: 1, ghz5: false, passive: false };
    let beacon = Beacon { tsf: 0, gp2: 0, interval: 100, dtim_period: 1, dtim_count: 0, rates: 0xf };
    let peer = Peer { bssid, channel, beacon };
    dev.join(&info, &peer).unwrap_or_else(|e| panic!("join: {e:?}, {:?}", failed(&r)));
    let mut auth = vec![0xb0, 0, 0, 0];
    auth.extend_from_slice(&bssid);
    auth.extend_from_slice(&MAC);
    auth.extend_from_slice(&bssid);
    auth.extend_from_slice(&[0, 0, 0, 0, 1, 0, 0, 0]);
    dev.send(&auth, false).unwrap();
    let answer = (0..100)
        .find_map(|_| {
            let n = dev.next(100_000).unwrap()?;
            let m = (n.code == msg::REPLY_RX_MPDU).then(|| msg::mpdu(&n.data)).flatten()?;
            (m.frame[0] == 0xb0).then_some(m)
        })
        .unwrap_or_else(|| panic!("no authentication answer, {:?}", failed(&r)));
    // To the station, transaction 2, success.
    assert_eq!((&answer.frame[4..10], answer.frame[26], answer.frame[28]), (&MAC[..], 2, 0));
    dev.drain(1_000_000);
    assert_eq!(dev.tx_failed(), 0);
    dev.associated(&info, &peer, 1).unwrap_or_else(|e| panic!("associated: {e:?}, {:?}", failed(&r)));
    dev.leave(&info, &peer);
    assert_eq!(failed(&r), None);
    // Joined again from nothing.
    dev.join(&info, &peer).unwrap();
    dev.leave(&info, &peer);
    assert_eq!(failed(&r), None);
}
