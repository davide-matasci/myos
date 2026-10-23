//! The modelled radio's firmware: its self load from the context
//! information (the sections checked against the synthetic file the
//! driver was given, `iwx::sim`), ALIVE, then the commands of the command
//! queue, each checked against the firmware's structure for it and
//! answered as the firmware does; the UMAC scan over the access points of
//! `air.rs` (their beacons heard on each channel's passive dwell), the
//! station contexts, and the frames of the station's queue sent to them,
//! their answers received.

use std::time::{Duration, Instant};

use iwx::sim;

use super::device::{le16, le32, le64, put16, put32, tbs, Device, Cause, RxRing, Ring, CMD_RING, MAC, TFD_LEN};
use super::air::Heard;

/// The ALIVE comes this long after the self load starts.
const LOAD_TIME: Duration = Duration::from_millis(20);
/// A beacon interval (100 TU).
const BEACON: Duration = Duration::from_micros(102_400);
/// The data queue the firmware gives the station (DQA's first after the
/// command queue).
const STA_QID: u8 = 1;

// The context information (struct iwx_context_info).
const CTXT_LEN: usize = 1792;
const DRAM: usize = 192;
const DRAM_ENTRIES: usize = 64;

// Groups, and the legacy commands' (sent as the long group).
const LONG: u8 = 1;
const SYSTEM: u8 = 2;
const MAC_CONF: u8 = 3;
const PHY_OPS: u8 = 4;
const DATA_PATH: u8 = 5;
const NVM: u8 = 0xc;

const FW_CTXT_INVALID: u32 = 0xffff_ffff;
const ACTION_ADD: u32 = 1;
const ACTION_MODIFY: u32 = 2;
const ACTION_REMOVE: u32 = 3;
const MAC_FILTER_ACCEPT_GRP: u32 = 1 << 2;
const STA_FLG_DRAIN_FLOW: u32 = 1 << 12;
const TX_STATUS_SUCCESS: u8 = 0x01;
const TX_STATUS_FAIL_SHORT_LIMIT: u8 = 0x82;
const SCAN_OFFLOAD_COMPLETED: u8 = 1;
const SCAN_FLAGS_NTFY_ITER_COMPLETE: u16 = 1 << 2;
const SCAN_FLAGS_FORCE_PASSIVE: u16 = 1 << 11;
const NVM_CHANNEL_VALID: u32 = 1 << 0;
const NVM_CHANNEL_IBSS: u32 = 1 << 1;
const NVM_CHANNEL_ACTIVE: u32 = 1 << 3;
const NVM_CHANNEL_RADAR: u32 = 1 << 4;

/// The 2.4 and 5 GHz channels in the NVM's channel profile order
/// (iwx_nvm_channels_8000).
const NVM_CHANNELS: [u8; 51] = [
    1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 36, 40, 44, 48, 52, 56, 60, 64, 68, 72, 76, 80, 84, 88, 92, 96, 100,
    104, 108, 112, 116, 120, 124, 128, 132, 136, 140, 144, 149, 153, 157, 161, 165, 169, 173, 177, 181,
];

/// What the regulatory domain allows on NVM channel `i`: 2.4 GHz 1-13,
/// 5 GHz UNII-1 and -3 actively, UNII-2 passively (radar).
fn channel_flags(i: usize) -> u32 {
    let ch = NVM_CHANNELS[i];
    let valid = NVM_CHANNEL_VALID | NVM_CHANNEL_IBSS;
    match ch {
        1..=13 | 36..=48 | 149..=165 => valid | NVM_CHANNEL_ACTIVE,
        52..=144 if !(68..=96).contains(&ch) => valid | NVM_CHANNEL_RADAR,
        _ => 0,
    }
}

#[derive(PartialEq, Eq)]
enum Stage {
    Loading(Instant),
    /// The NVM access commands the driver announced, until it says it is
    /// done with them (INIT_COMPLETE).
    Booted { nvm: bool },
    Running,
}

struct Scan {
    channels: Vec<u8>,
    dwell: Duration,
    i: usize,
    /// Its beacons heard on this channel yet; when it moves on.
    heard: bool,
    hear_at: Instant,
    leave_at: Instant,
    notify_iter: bool,
}

#[derive(Clone, Copy)]
struct Phy {
    channel: u8,
    ghz5: bool,
}

struct MacCtx {
    node: [u8; 6],
    bssid: [u8; 6],
    filter: u32,
    assoc: bool,
}

struct Sta {
    drain: bool,
}

pub struct Fw {
    stage: Stage,
    cmd: Ring,
    tx_chains: u32,
    scan: Option<Scan>,
    phy: Option<Phy>,
    mac: Option<MacCtx>,
    bound: bool,
    sta: Option<Sta>,
    txq: Option<Ring>,
    protected: bool,
    tlc: bool,
    next_beacon: Instant,
}

/// UREG_CPU_INIT_RUN: the firmware loads itself as the context
/// information at `ctxt` says, which must be the synthetic file's
/// sections, each where its DRAM entry points.
pub fn start(d: &mut Device, ctxt: u64) -> Result<(), String> {
    if ctxt == 0 || ctxt % 4096 != 0 {
        return Err(format!("the self load started with the context information at 0x{ctxt:x}"));
    }
    let c = d.dma_read(ctxt, CTXT_LEN, "context information").ok_or("")?;
    let flags = le32(&c, 8);
    // Version: the MAC id, version 0, the size in words; long TFDs, 2^9
    // receive buffers of 4 KB.
    if le16(&c, 0) != d.hw.hw_rev_raw as u16 || le16(&c, 2) != 0 || le16(&c, 4) != (CTXT_LEN / 4) as u16 {
        return Err(format!("context information version {:x?}", &c[..8]));
    }
    if flags != (1 << 8) | (9 << 4) | (4 << 9) {
        return Err(format!("context information flags 0x{flags:x}: not long TFDs and 512 receive buffers of 4 KB"));
    }
    let (free_rbd, used_rbd, status, cmd_queue) = (le64(&c, 24), le64(&c, 32), le64(&c, 40), le64(&c, 48));
    for (what, a, align) in [("free RBDs", free_rbd, 256), ("used RBDs", used_rbd, 256), ("RX status", status, 16), ("command queue", cmd_queue, 256)] {
        if a == 0 || a % align != 0 {
            return Err(format!("context information: {what} at 0x{a:x}"));
        }
    }
    if c[56] != (CMD_RING.trailing_zeros() - 3) as u8 {
        return Err(format!("context information: command queue size {} (log2 - 3)", c[56]));
    }
    // The DRAM map: UMAC, LMAC, paging, in the file's order (k counts the
    // sections LMAC first, as iwx::sim::byte does).
    let entries = |image: usize| {
        (0..DRAM_ENTRIES).map(|i| le64(&c, DRAM + 8 * (DRAM_ENTRIES * image + i))).take_while(|&a| a != 0).collect::<Vec<_>>()
    };
    let lmac_k = 0;
    let umac_k = sim::LMAC.len();
    let paging_k = umac_k + sim::UMAC.len();
    for (name, image, sections, k0) in [("UMAC", 0, &sim::UMAC[..], umac_k), ("LMAC", 1, &sim::LMAC[..], lmac_k), ("paging", 2, &sim::PAGING[..], paging_k)] {
        let e = entries(image);
        if e.len() != sections.len() {
            return Err(format!("{} {name} sections in the context information, the simulated firmware has {}", e.len(), sections.len()));
        }
        for (i, (&addr, s)) in e.iter().zip(sections).enumerate() {
            if addr % 4096 != 0 {
                return Err(format!("{name} section {i} at 0x{addr:x}, not page aligned"));
            }
            let data = d.dma_read(addr, s.len, "firmware section").ok_or("")?;
            if let Some(at) = (0..s.len).find(|&j| data[j] != sim::byte(k0 + i, j)) {
                return Err(format!("{name} section {i} differs at byte {at}: not the simulated firmware (iwx::sim)"));
            }
        }
    }
    d.rx = Some(RxRing::new(free_rbd, used_rbd, status));
    d.fw = Some(Fw {
        stage: Stage::Loading(Instant::now() + LOAD_TIME),
        cmd: Ring { tfds: cmd_queue, byte_counts: None, read: 0 },
        tx_chains: 0,
        scan: None,
        phy: None,
        mac: None,
        bound: false,
        sta: None,
        txq: None,
        protected: false,
        tlc: false,
        next_beacon: Instant::now(),
    });
    Ok(())
}

fn fw(d: &mut Device) -> &mut Fw {
    d.fw.as_mut().expect("the firmware runs")
}

/// What the firmware does by itself: ALIVE once loaded, the scan's
/// channels, the joined network's beacons. When to come back.
pub fn tick(d: &mut Device, now: Instant) -> Duration {
    let f = fw(d);
    if let Stage::Loading(at) = f.stage {
        if now < at {
            return at - now;
        }
        f.stage = Stage::Booted { nvm: false };
        d.raise(Cause::Alive);
        // Into the receive ring once the driver gives it buffers (it does
        // at ALIVE).
        d.send([0x01, 0, 0, 0x80], &alive(), true);
        return Duration::from_millis(20);
    }
    let mut next = Duration::from_millis(20);
    if let Some(n) = scan_tick(d, now) {
        next = next.min(n);
    }
    let f = fw(d);
    if f.scan.is_none() && f.bound {
        if now >= f.next_beacon {
            f.next_beacon = now + BEACON;
            let channel = f.phy.map_or(0, |p| p.channel);
            let beacons: Vec<Heard> = d.air.beacons().into_iter().filter(|h| h.channel == channel).collect();
            for h in beacons {
                hear(d, h);
            }
        }
        next = next.min(fw(d).next_beacon.saturating_duration_since(now));
    }
    next
}

/// iwx_alive_resp_v6: ALIVE_OK, the LMAC's error table where the driver
/// reads it.
fn alive() -> Vec<u8> {
    let mut a = vec![0u8; 144];
    put16(&mut a, 0, 0xcafe);
    put32(&mut a, 4, 77);
    put32(&mut a, 4 + 16, sim::LMAC_ERROR_TABLE);
    put32(&mut a, 100, 77);
    put32(&mut a, 100 + 8, 0x0080_2000);
    a
}

/// A queue's write pointer moved to `idx`: the descriptors up to it.
pub fn kick(d: &mut Device, qid: u8, idx: usize) {
    let Some(f) = d.fw.as_mut() else {
        d.violation(format!("queue {qid} kicked before the firmware runs"));
        return;
    };
    if matches!(f.stage, Stage::Loading(_)) {
        d.violation(format!("queue {qid} kicked before ALIVE"));
        return;
    }
    loop {
        if d.failed.is_some() {
            return;
        }
        let f = fw(d);
        let ring = match qid {
            0 => &mut f.cmd,
            STA_QID if f.txq.is_some() => f.txq.as_mut().unwrap(),
            _ => {
                d.violation(format!("queue {qid} kicked: the firmware has no such queue"));
                return;
            }
        };
        if ring.read == idx {
            return;
        }
        let (i, tfds) = (ring.read, ring.tfds);
        ring.read = (i + 1) % CMD_RING;
        let Some(tfd) = d.dma_read(tfds + (i * TFD_LEN) as u64, TFD_LEN, "descriptor") else {
            return;
        };
        if qid == 0 {
            command(d, i, &tfd);
        } else {
            transmit(d, qid, i, &tfd);
        }
    }
}

/// The transfer buffers of a descriptor, gathered.
fn gather(d: &mut Device, tbs: &[(u64, usize)]) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    for &(addr, len) in tbs {
        out.extend(d.dma_read(addr, len, "transfer buffer")?);
    }
    Some(out)
}

/// The answer to a command: its data, or the firmware refusing it; the
/// notifications that follow it.
struct Answer {
    data: Vec<u8>,
    failed: bool,
    then: Vec<([u8; 4], Vec<u8>)>,
}

impl Answer {
    fn ok(data: Vec<u8>) -> Result<Answer, String> {
        Ok(Answer { data, failed: false, then: Vec::new() })
    }

    fn empty() -> Result<Answer, String> {
        Answer::ok(Vec::new())
    }
}

/// The command at index `i` of the command queue.
fn command(d: &mut Device, i: usize, tfd: &[u8]) {
    let t = tbs(tfd);
    if t.is_empty() || t.len() > 2 {
        d.violation(format!("command {i}: {} transfer buffers", t.len()));
        return;
    }
    let Some(c) = gather(d, &t) else {
        return;
    };
    // The first 20 bytes (the header and the start) in the first buffer.
    if t[0].1 != c.len().min(20) {
        d.violation(format!("command {i}: its first transfer buffer is {} bytes of {}", t[0].1, c.len()));
        return;
    }
    if c.len() < 8 {
        d.violation(format!("command {i}: {} bytes, short of its header", c.len()));
        return;
    }
    let (opcode, group, idx, qid, len, version) = (c[0], c[1], c[2], c[3], le16(&c, 4) as usize, c[7]);
    if qid != 0 || idx as usize != i || len != c.len() - 8 {
        d.violation(format!("command {i} (group {group} opcode 0x{opcode:x}): header says queue {qid} index {idx}, {len} bytes of {}", c.len() - 8));
        return;
    }
    if group == 0 {
        d.violation(format!("command 0x{opcode:x} in the legacy group: this firmware takes it in the long group"));
        return;
    }
    let declared = sim::CMD_VERSIONS.iter().find(|v| v[0] == opcode && v[1] == group).map(|v| v[2]);
    if version != 0 && Some(version) != declared {
        d.violation(format!("command group {group} opcode 0x{opcode:x} version {version}, the firmware has {declared:?}"));
        return;
    }
    let payload = c[8..].to_vec();
    match handle(d, group, opcode, &payload) {
        Ok(a) => {
            let g = if a.failed { group | 0x40 } else { group };
            d.send([opcode, g, idx, 0], &a.data, true);
            for (hdr, data) in a.then {
                d.send(hdr, &data, true);
            }
        }
        Err(e) => d.violation(format!("command group {group} opcode 0x{opcode:x}: {e}")),
    }
}

fn len_is(p: &[u8], n: usize) -> Result<(), String> {
    if p.len() == n { Ok(()) } else { Err(format!("{} bytes, the firmware's structure is {n}", p.len())) }
}

fn handle(d: &mut Device, group: u8, opcode: u8, p: &[u8]) -> Result<Answer, String> {
    let (mac, hw) = (MAC, d.hw);
    let f = fw(d);
    // Until INIT_COMPLETE only the NVM access commands.
    match (&f.stage, group, opcode) {
        (Stage::Booted { .. }, SYSTEM, 0x03) | (Stage::Booted { .. }, NVM, 0x00) | (Stage::Running, _, _) => {}
        _ => return Err("before the firmware's INIT_COMPLETE".into()),
    }
    match (group, opcode) {
        // INIT_EXTENDED_CFG: NVM access commands follow.
        (SYSTEM, 0x03) => {
            len_is(p, 4)?;
            f.stage = Stage::Booted { nvm: le32(p, 0) & (1 << 1) != 0 };
            Answer::empty()
        }
        // NVM_ACCESS_COMPLETE: INIT_COMPLETE follows.
        (NVM, 0x00) => {
            len_is(p, 4)?;
            if f.stage != (Stage::Booted { nvm: true }) {
                return Err("NVM access complete without INIT_EXTENDED_CFG's NVM flag".into());
            }
            f.stage = Stage::Running;
            Ok(Answer { data: Vec::new(), failed: false, then: vec![([0x04, 0, 0, 0x80], Vec::new())] })
        }
        (NVM, 0x02) => {
            len_is(p, 4)?;
            Answer::ok(nvm_info())
        }
        // TX_ANT_CONFIGURATION: chains the SKU has.
        (LONG, 0x98) => {
            len_is(p, 4)?;
            let valid = le32(p, 0);
            let sku = (sim::PHY_SKU >> 16) & 0xf;
            if valid == 0 || valid & !sku != 0 {
                return Err(format!("transmit antennas 0x{valid:x}, the SKU has 0x{sku:x}"));
            }
            f.tx_chains = valid;
            Answer::empty()
        }
        // BT_CONFIG.
        (LONG, 0x9b) => {
            len_is(p, 8)?;
            if le32(p, 0) > 3 {
                return Err(format!("Bluetooth coexistence mode {}", le32(p, 0)));
            }
            Answer::empty()
        }
        // SOC_CONFIGURATION: DISCRETE as the radio is (no other flag), its
        // crystal's latency.
        (SYSTEM, 0x01) => {
            len_is(p, 8)?;
            let (flags, latency) = (le32(p, 0), le32(p, 4));
            if flags != u32::from(!hw.integrated) || latency != hw.xtal_latency_us {
                return Err(format!(
                    "the SoC configured with flags 0x{flags:x}, {latency} µs: the {} is {}, {} µs",
                    hw.name,
                    if hw.integrated { "integrated" } else { "discrete" },
                    hw.xtal_latency_us
                ));
            }
            Answer::empty()
        }
        (PHY_OPS, 0x04) => {
            len_is(p, 20)?;
            if le32(p, 0) > 8 {
                return Err(format!("{} temperature thresholds of at most 8", le32(p, 0)));
            }
            Answer::empty()
        }
        // POWER_TABLE.
        (LONG, 0x77) => {
            len_is(p, 4)?;
            Answer::empty()
        }
        // MCC_UPDATE: answered (version 4) with the domain and the
        // channels it allows.
        (LONG, 0xc8) => {
            len_is(p, 28)?;
            let mut r = vec![0u8; 20 + 4 * NVM_CHANNELS.len()];
            put16(&mut r, 4, le16(p, 0));
            put32(&mut r, 16, NVM_CHANNELS.len() as u32);
            for i in 0..NVM_CHANNELS.len() {
                put32(&mut r, 20 + 4 * i, channel_flags(i));
            }
            Answer::ok(r)
        }
        // SCAN_CFG, the reduced form.
        (LONG, 0x0c) => {
            len_is(p, 12)?;
            let (tx, rx) = (le32(p, 4), le32(p, 8));
            if tx == 0 || tx & !3 != 0 || rx == 0 || rx & !3 != 0 {
                return Err(format!("scan chains tx 0x{tx:x} rx 0x{rx:x}"));
            }
            Answer::empty()
        }
        (LONG, 0x0d) => scan_request(d, p),
        // PHY_CONTEXT version 4 (ultra-high-band channel information).
        (LONG, 0x08) => {
            len_is(p, 32)?;
            let (id, action, channel, band, width) = (le32(p, 0), le32(p, 4), le32(p, 8), p[12], p[13]);
            if id != 0 {
                return Err(format!("PHY context {id:x}: the driver's is 0"));
            }
            match action {
                ACTION_ADD | ACTION_MODIFY => {
                    let ch = valid_channel(channel, band)?;
                    if width != 0 {
                        return Err(format!("PHY context width {width}: 20 MHz only"));
                    }
                    if (action == ACTION_ADD) == f.phy.is_some() {
                        return Err("PHY context added twice, or modified before it was added".into());
                    }
                    f.phy = Some(Phy { channel: ch, ghz5: band == 0 });
                }
                ACTION_REMOVE => {
                    if f.phy.is_none() || f.bound {
                        return Err("PHY context removed while bound or not there".into());
                    }
                    f.phy = None;
                }
                a => return Err(format!("PHY context action {a}")),
            }
            Answer::empty()
        }
        // RLC_CONFIG version 2: the PHY context's receive chains.
        (DATA_PATH, 0x08) => {
            len_is(p, 32)?;
            if f.phy.is_none() || le32(p, 0) != 0 {
                return Err(format!("RLC configuration of PHY context {} the firmware does not have", le32(p, 0)));
            }
            Answer::empty()
        }
        (LONG, 0x28) => mac_context(f, mac, p),
        // BINDING_CONTEXT: MAC context 0 to PHY context 0.
        (LONG, 0x2b) => {
            len_is(p, 28)?;
            let (action, macs, phy) = (le32(p, 4), [le32(p, 8), le32(p, 12), le32(p, 16)], le32(p, 20));
            match action {
                ACTION_ADD => {
                    if f.bound || f.mac.is_none() || f.phy.is_none() || macs != [0, FW_CTXT_INVALID, FW_CTXT_INVALID] || phy != 0 {
                        return Err(format!("binding of MACs {macs:x?} to PHY {phy}, with the contexts {}", if f.bound { "bound" } else { "missing" }));
                    }
                    f.bound = true;
                }
                ACTION_REMOVE => {
                    if !f.bound {
                        return Err("binding removed, not there".into());
                    }
                    f.bound = false;
                }
                a => return Err(format!("binding action {a}")),
            }
            Answer::ok(0u32.to_le_bytes().to_vec())
        }
        (LONG, 0x18) => add_sta(f, p),
        // REMOVE_STA: its queue gone first.
        (LONG, 0x19) => {
            len_is(p, 4)?;
            if p[0] != 0 || f.sta.is_none() || f.txq.is_some() {
                return Err(format!("station {} removed: not there, or its queue still is", p[0]));
            }
            f.sta = None;
            Answer::empty()
        }
        // TXPATH_FLUSH: nothing waits (frames go when kicked).
        (LONG, 0x1e) => {
            len_is(p, 8)?;
            if le32(p, 0) != 0 || f.sta.is_none() {
                return Err(format!("flush of station {}, which the firmware does not have", le32(p, 0)));
            }
            Answer::empty()
        }
        (DATA_PATH, 0x17) => queue_config(f, p),
        // SESSION_PROTECTION (MAC context 0's association).
        (MAC_CONF, 0x05) => {
            len_is(p, 24)?;
            let (id, action, conf, duration) = (le32(p, 0), le32(p, 4), le32(p, 8), le32(p, 12));
            if id != 0 || f.mac.is_none() || conf != 0 {
                return Err(format!("session protection of MAC {id} conf {conf}"));
            }
            match action {
                ACTION_ADD if duration > 0 => f.protected = true,
                ACTION_REMOVE => f.protected = false,
                a => return Err(format!("session protection action {a}, {duration} TU")),
            }
            Answer::empty()
        }
        // SF_CFG: full on (1) or the initial state (3)... while
        // associated.
        (LONG, 0xd1) => {
            len_is(p, 92)?;
            if !matches!(le32(p, 0), 1 | 3) {
                return Err(format!("smart FIFO state {}", le32(p, 0)));
            }
            Answer::empty()
        }
        // MCAST_FILTER: no address list.
        (LONG, 0xd0) => {
            len_is(p, 12)?;
            if p[2] != 0 {
                return Err(format!("{} multicast addresses and no list", p[2]));
            }
            Answer::empty()
        }
        (LONG, 0xa9) => {
            len_is(p, 40)?;
            if le32(p, 0) != 0 || f.mac.is_none() {
                return Err(format!("power settings of MAC {}", le32(p, 0)));
            }
            Answer::empty()
        }
        // TLC_MNG_CONFIG version 4: the station's rates, chain A.
        (DATA_PATH, 0x0f) => {
            len_is(p, 28)?;
            if p[0] != 0 || f.sta.is_none() || p[6] == 0 || le16(p, 10) == 0 {
                return Err(format!("rate scaling of station {} (chains {}, rates 0x{:x})", p[0], p[6], le16(p, 10)));
            }
            f.tlc = true;
            Answer::empty()
        }
        _ => Err("a command this firmware does not have".into()),
    }
}

/// NVM_GET_INFO's answer (iwx_nvm_get_info_rsp version 4): both bands,
/// chains A and B, LAR, the channel profile.
fn nvm_info() -> Vec<u8> {
    let mut r = vec![0u8; 468];
    put16(&mut r, 4, 0x0a1d);
    r[7] = 1; // one MAC address
    put32(&mut r, 8, 0x1f); // 2.4 GHz, 5.2 GHz, 11n, 11ac, 11ax
    put32(&mut r, 12, 3);
    put32(&mut r, 16, 3);
    put32(&mut r, 20, 1); // LAR enabled
    put32(&mut r, 24, NVM_CHANNELS.len() as u32);
    for i in 0..NVM_CHANNELS.len() {
        put32(&mut r, 28 + 4 * i, channel_flags(i));
    }
    r
}

/// A channel `ch` on `band` (0: 5 GHz, 1: 2.4 GHz) the domain allows.
fn valid_channel(ch: u32, band: u8) -> Result<u8, String> {
    let i = NVM_CHANNELS.iter().position(|&c| u32::from(c) == ch);
    match i {
        Some(i) if channel_flags(i) != 0 && band == u8::from(ch <= 14) => Ok(ch as u8),
        _ => Err(format!("channel {ch} on band {band}: not one the regulatory domain allows")),
    }
}

/// MAC_CONTEXT: the station's (BSS_STA, id 0).
fn mac_context(f: &mut Fw, mac: [u8; 6], p: &[u8]) -> Result<Answer, String> {
    len_is(p, 148)?;
    let (id, action) = (le32(p, 0), le32(p, 4));
    if id != 0 {
        return Err(format!("MAC context {id:x}: the driver's is 0"));
    }
    match action {
        ACTION_ADD | ACTION_MODIFY => {
            if (action == ACTION_ADD) == f.mac.is_some() {
                return Err("MAC context added twice, or modified before it was added".into());
            }
            if le32(p, 8) != 5 {
                return Err(format!("MAC type {}: a station's is BSS_STA (5)", le32(p, 8)));
            }
            let node: [u8; 6] = p[16..22].try_into().unwrap();
            if node != mac {
                return Err(format!("MAC context address {node:02x?}, the device's is {mac:02x?}"));
            }
            let assoc = le32(p, 100) != 0;
            if assoc && (le32(p, 136) == 0 || f.sta.is_none()) {
                return Err(format!("associated as {} with no station", le32(p, 136)));
            }
            let bssid = p[24..30].try_into().unwrap();
            f.mac = Some(MacCtx { node, bssid, filter: le32(p, 52), assoc });
        }
        ACTION_REMOVE => {
            if f.mac.is_none() || f.bound || f.sta.is_some() {
                return Err("MAC context removed while bound, with its station, or not there".into());
            }
            f.mac = None;
            f.protected = false;
        }
        a => return Err(format!("MAC context action {a}")),
    }
    Answer::empty()
}

/// ADD_STA (version 12): the access point as station 0, or a change to
/// it (its drain flag). Answered ADD_STA_SUCCESS.
fn add_sta(f: &mut Fw, p: &[u8]) -> Result<Answer, String> {
    len_is(p, 48)?;
    let (modify, mac_id, sta_id) = (p[0], le32(p, 4), p[16]);
    let (flags, mask) = (le32(p, 20), le32(p, 24));
    if sta_id != 0 || mac_id != 0 || f.mac.is_none() {
        return Err(format!("station {sta_id} of MAC {mac_id}, with no such MAC context"));
    }
    match modify {
        0 => {
            if f.sta.is_some() || !f.bound {
                return Err("station added twice, or before the binding".into());
            }
            let addr: [u8; 6] = p[8..14].try_into().unwrap();
            if f.mac.as_ref().is_some_and(|m| m.bssid != addr) {
                return Err(format!("station {addr:02x?}, the MAC context's BSSID is another"));
            }
            f.sta = Some(Sta { drain: false });
        }
        1 => {
            let Some(s) = f.sta.as_mut() else {
                return Err("station modified before it was added".into());
            };
            if mask & STA_FLG_DRAIN_FLOW != 0 {
                s.drain = flags & STA_FLG_DRAIN_FLOW != 0;
            }
        }
        m => return Err(format!("ADD_STA add_modify {m}")),
    }
    Answer::ok(1u32.to_le_bytes().to_vec())
}

/// SCD_QUEUE_CONFIG version 3: the station's queue added (its ring and
/// byte counts where the driver says, 256 descriptors) or removed.
fn queue_config(f: &mut Fw, p: &[u8]) -> Result<Answer, String> {
    len_is(p, 36)?;
    let (op, sta_mask, tid) = (le32(p, 0), le32(p, 4), p[8]);
    if sta_mask != 1 || f.sta.is_none() {
        return Err(format!("queue of stations 0x{sta_mask:x}, which the firmware does not have"));
    }
    match op {
        0 => {
            let (flags, cb_size, bc, tfdq) = (le32(p, 12), le32(p, 16), le64(p, 20), le64(p, 28));
            if f.txq.is_some() || flags != 0 || cb_size != 5 || bc == 0 || bc % 64 != 0 || tfdq == 0 || tfdq % 256 != 0 {
                return Err(format!("queue added (TID {tid}, flags 0x{flags:x}, size {cb_size}, byte counts 0x{bc:x}, descriptors 0x{tfdq:x})"));
            }
            f.txq = Some(Ring { tfds: tfdq, byte_counts: Some(bc), read: 0 });
            let mut r = vec![0u8; 8];
            put16(&mut r, 0, u16::from(STA_QID));
            Answer::ok(r)
        }
        1 => {
            if f.txq.take().is_none() {
                return Err("queue removed, not there".into());
            }
            Answer::empty()
        }
        o => Err(format!("queue operation {o}")),
    }
}

/// SCAN_REQ_UMAC version 14 (the layout of versions 14 to 16): a passive
/// scan of its channels, each for the passive dwell.
fn scan_request(d: &mut Device, p: &[u8]) -> Result<Answer, String> {
    len_is(p, 1940)?;
    let f = fw(d);
    if f.scan.is_some() {
        return Ok(Answer { data: Vec::new(), failed: true, then: Vec::new() });
    }
    let (g, ch) = (8, 44);
    let flags = le16(p, g);
    if flags & SCAN_FLAGS_FORCE_PASSIVE == 0 {
        return Err("an active scan: the firmware would need the probe request's SSIDs".into());
    }
    let dwell = p[g + 32];
    if dwell == 0 {
        return Err("a passive dwell of 0 TU".into());
    }
    let n = p[ch + 1] as usize;
    if n == 0 || n > 67 {
        return Err(format!("{n} channels, of 1 to 67"));
    }
    let mut channels = Vec::new();
    for i in 0..n {
        let e = ch + 4 + 8 * i;
        channels.push(valid_channel(u32::from(p[e + 4]), p[e + 5])?);
        if p[e + 6] == 0 {
            return Err(format!("channel {} scanned 0 times", p[e + 4]));
        }
    }
    let now = Instant::now();
    let dwell = Duration::from_micros(u64::from(dwell) * 1024);
    f.scan = Some(Scan {
        channels,
        dwell,
        i: 0,
        heard: false,
        hear_at: now + dwell / 2,
        leave_at: now + dwell,
        notify_iter: flags & SCAN_FLAGS_NTFY_ITER_COMPLETE != 0,
    });
    Answer::empty()
}

/// The scan: each channel's beacons half way through its dwell, the next
/// channel at its end; the end notified.
fn scan_tick(d: &mut Device, now: Instant) -> Option<Duration> {
    let f = fw(d);
    let s = f.scan.as_mut()?;
    if !s.heard && now >= s.hear_at {
        s.heard = true;
        let ch = s.channels[s.i];
        let beacons: Vec<Heard> = d.air.beacons().into_iter().filter(|h| h.channel == ch).collect();
        for h in beacons {
            mpdu(d, &h);
        }
    }
    let f = fw(d);
    let s = f.scan.as_mut()?;
    if now >= s.leave_at {
        s.i += 1;
        if s.i == s.channels.len() {
            let notify = s.notify_iter;
            f.scan = None;
            if notify {
                // iwx_lmac_scan_complete_notif's successor: status, the
                // channels scanned.
                let mut it = vec![0u8; 16];
                it[0] = SCAN_OFFLOAD_COMPLETED;
                d.send([0xb5, 0, 0, 0x80], &it, false);
            }
            let mut c = vec![0u8; 16];
            c[6] = SCAN_OFFLOAD_COMPLETED;
            d.send([0x0f, 0, 0, 0x80], &c, true);
            return None;
        }
        (s.heard, s.hear_at, s.leave_at) = (false, now + s.dwell / 2, now + s.dwell);
    }
    Some(if s.heard { s.leave_at } else { s.hear_at }.saturating_duration_since(now))
}

/// A frame on the channel the station is bound to: up to the host when it
/// is for it (its address, a group address the MAC context accepts, a
/// beacon).
fn hear(d: &mut Device, h: Heard) {
    let f = fw(d);
    let Some(m) = f.mac.as_ref() else {
        return;
    };
    let ra: [u8; 6] = h.frame[4..10].try_into().unwrap();
    let beacon = h.frame[0] == 0x80;
    let group = ra[0] & 1 != 0;
    if ra == m.node || beacon || (group && m.filter & MAC_FILTER_ACCEPT_GRP != 0) {
        mpdu(d, &h);
    }
}

/// REPLY_RX_MPDU (iwx_rx_mpdu_desc v1): the frame with its CRC good, the
/// MAC header padded to 4 bytes when it is not, its signal on antenna A,
/// the channel, the device's clock.
fn mpdu(d: &mut Device, h: &Heard) {
    let f = &h.frame;
    let hdr = iwx::msg::header_len(f).min(f.len());
    let pad = f[0] & 0x0c != 0x04 && hdr % 4 != 0;
    let mut frame = f[..hdr].to_vec();
    if pad {
        frame.extend_from_slice(&[0, 0]);
    }
    frame.extend_from_slice(&f[hdr..]);
    let mut p = vec![0u8; 48];
    put16(&mut p, 0, frame.len() as u16);
    p[3] = if pad { 0x20 } else { 0 };
    put32(&mut p, 12, 0b11); // CRC_OK, OVERRUN_OK
    p[32] = h.rssi.unsigned_abs();
    p[34] = h.channel;
    let gp2 = d.gp2();
    put32(&mut p, 36, gp2);
    put32(&mut p, 40, gp2);
    p.extend_from_slice(&frame);
    d.send([0xc1, 0, 0, 0x80], &p, false);
}

/// The frame at index `i` of the station's queue: its TX command checked
/// against its descriptor and byte count, sent on the bound channel to
/// the access points, its TX response, then their answers.
fn transmit(d: &mut Device, qid: u8, i: usize, tfd: &[u8]) {
    if let Err(e) = send_frame(d, qid, i, tfd) {
        d.violation(format!("frame {i} of queue {qid}: {e}"));
    }
}

fn send_frame(d: &mut Device, qid: u8, i: usize, tfd: &[u8]) -> Result<(), String> {
    const CMD_RATE: u32 = 1 << 0;
    const ENCRYPT_DIS: u32 = 1 << 1;
    const OFFLD_PAD: u16 = 1 << 13;
    let t = tbs(tfd);
    if t.len() < 2 || t[0].1 != 20 {
        return Err(format!("transfer buffers {t:x?}: the TX command's first 20 bytes, then the rest"));
    }
    let c = gather(d, &t[..2]).ok_or("")?;
    let body = gather(d, &t[2..]).ok_or("")?;
    if c.len() < 24 || c[..4] != [0x1c, 0, i as u8, qid] {
        return Err(format!("TX command header {:x?}", &c[..4.min(c.len())]));
    }
    let (len, offload, flags, rate) = (le16(&c, 4) as usize, le16(&c, 6), le32(&c, 8), le32(&c, 20));
    let hdr = usize::from((offload >> 8) & 0x1f) * 2;
    let pad = if offload & OFFLD_PAD != 0 { 2 } else { 0 };
    if hdr < 24 || c.len() != 24 + hdr + pad || (24 + hdr + pad) % 4 != 0 {
        return Err(format!("TX command of {} bytes for a {hdr}-byte header (pad {pad})", c.len()));
    }
    let mut frame = c[24..24 + hdr].to_vec();
    frame.extend_from_slice(&body);
    if frame.len() != len || iwx::msg::header_len(&frame) != hdr {
        return Err(format!("frame of {} bytes ({}-byte header), the TX command says {len} ({hdr})", frame.len(), iwx::msg::header_len(&frame)));
    }
    let f = fw(d);
    let ring = f.txq.as_ref().ok_or("no queue")?;
    let bc_at = ring.byte_counts.unwrap() + 2 * i as u64;
    let want = (len.div_ceil(4) as u16) | (((2 + 10 * t.len()).div_ceil(64) - 1) as u16) << 12;
    let bc = d.dma_read(bc_at, 2, "byte count").ok_or("")?;
    if le16(&bc, 0) != want {
        return Err(format!("byte count 0x{:x}, the descriptor's is 0x{want:x}", le16(&bc, 0)));
    }
    if frame[1] & 0x40 != 0 && flags & ENCRYPT_DIS == 0 {
        return Err("a protected frame for the firmware to encrypt, with no key".into());
    }
    let f = fw(d);
    let phy = f.phy.ok_or("no PHY context")?;
    if !f.bound || f.sta.is_none() {
        return Err("sent with no binding or station".into());
    }
    if flags & CMD_RATE != 0 {
        let ofdm = rate & (7 << 8) == 1 << 8;
        if rate & (1 << 14) == 0 || rate & 0xff != 0 || (rate & (7 << 8) != 0 && !ofdm) || (phy.ghz5 && !ofdm) {
            return Err(format!("rate 0x{rate:x} on channel {}", phy.channel));
        }
    } else if !f.tlc {
        return Err("no rate, and the firmware's rate scaling not configured".into());
    }
    let ra: [u8; 6] = frame[4..10].try_into().unwrap();
    let drained = f.sta.as_ref().is_some_and(|s| s.drain);
    let on_channel = f.protected || f.mac.as_ref().is_some_and(|m| m.assoc);
    let mut heard = Vec::new();
    let acked = if drained || !on_channel {
        false
    } else {
        heard = d.air.hear(phy.channel, &frame);
        ra[0] & 1 != 0 || d.air.has_ap(phy.channel, ra)
    };
    // iwx_tx_resp: one frame, its status; the next index to reclaim.
    let mut r = vec![0u8; 48];
    r[0] = 1;
    put16(&mut r, 28, le16(&frame, 22));
    put16(&mut r, 30, len as u16);
    put16(&mut r, 34, le16(&frame, 0));
    put16(&mut r, 36, u16::from(qid));
    r[40] = if acked { TX_STATUS_SUCCESS } else { TX_STATUS_FAIL_SHORT_LIMIT };
    put32(&mut r, 44, ((i + 1) % CMD_RING) as u32);
    d.send([0x1c, 0, i as u8, qid], &r, true);
    for h in heard {
        hear(d, h);
    }
    Ok(())
}
