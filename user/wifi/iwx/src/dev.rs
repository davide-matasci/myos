//! The device: its memory shared with the firmware, the firmware's load, the
//! interrupt, commands and notifications, the bring-up up to a scan, then
//! joining a network and sending frames (from OpenBSD's if_iwx.c:
//! iwx_ctxt_info_init, iwx_init_fw_sec, iwx_start_fw, iwx_intr,
//! iwx_intr_msix, iwx_notif_intr, iwx_send_cmd, iwx_run_init_mvm_ucode,
//! iwx_init_hw, iwx_auth, iwx_run, iwx_run_stop, iwx_deauth, iwx_tx,
//! iwx_rx_tx_cmd; notices in LICENSE).

use alloc::collections::VecDeque;
use alloc::vec::Vec;
use core::sync::atomic::{Ordering, fence};

use crate::fw::{Firmware, Section};
use crate::hw::{Bus, Hw, HwError};
use crate::msg::{self, Channel, NvmInfo};
use crate::reg::*;
use crate::ring::{self, CmdId, CtxtAddrs, Packet};
use crate::sta::{self, Action, Assoc, Beacon};

/// What the driver needs of the platform beyond the registers.
pub trait Device: Bus {
    /// A zeroed, physically contiguous buffer the device reaches by DMA,
    /// `len` a multiple of 4 KB (the driver asks for at most 11).
    fn dma(&mut self, len: usize) -> Option<Dma>;
    /// Wait up to `timeout_us` for the device's interrupt: whether it came
    /// (since the last wait).
    fn wait_irq(&mut self, timeout_us: u64) -> bool;
    fn now_us(&mut self) -> u64;
    /// The interrupt was serviced: an INTx line, masked by the platform
    /// from its interrupt on, may be unmasked (the device is quiet).
    fn irq_done(&mut self) {}
}

/// A DMA buffer: mapped at `ptr` for the driver, at `phys` for the device.
pub struct Dma {
    ptr: *mut u8,
    len: usize,
    phys: u64,
}

impl Dma {
    /// # Safety
    /// `ptr` points to `len` bytes of memory the device sees at `phys`,
    /// valid as long as the `Dma` and touched by nothing else on the host.
    pub unsafe fn new(ptr: *mut u8, len: usize, phys: u64) -> Self {
        Dma { ptr, len, phys }
    }

    pub fn phys(&self) -> u64 {
        self.phys
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// What the device wrote, after it said so (an interrupt, a status).
    pub fn read(&self, off: usize, out: &mut [u8]) {
        assert!(off + out.len() <= self.len);
        fence(Ordering::Acquire);
        // SAFETY: in bounds (above); the memory is ours (Dma::new).
        unsafe { core::ptr::copy_nonoverlapping(self.ptr.add(off), out.as_mut_ptr(), out.len()) }
    }

    /// For the device, before telling it (a register write).
    pub fn write(&mut self, off: usize, data: &[u8]) {
        assert!(off + data.len() <= self.len);
        // SAFETY: as in read.
        unsafe { core::ptr::copy_nonoverlapping(data.as_ptr(), self.ptr.add(off), data.len()) }
        fence(Ordering::Release);
    }

    pub fn read32(&self, off: usize) -> u32 {
        assert!(off + 4 <= self.len && off % 4 == 0);
        // SAFETY: in bounds and aligned (the buffer is page aligned).
        u32::from_le(unsafe { core::ptr::read_volatile(self.ptr.add(off) as *const u32) })
    }

    pub fn write32(&mut self, off: usize, v: u32) {
        assert!(off + 4 <= self.len && off % 4 == 0);
        // SAFETY: as in read32.
        unsafe { core::ptr::write_volatile(self.ptr.add(off) as *mut u32, v.to_le()) }
    }

    pub fn write64(&mut self, off: usize, v: u64) {
        self.write(off, &v.to_le_bytes());
    }
}

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum Error {
    Hw(HwError),
    /// Not an AX201 (a QuZ MAC): its HW_REV, as the driver reads it.
    Unsupported(u32),
    /// No DMA memory, or the firmware's images do not fit what the
    /// context information has room for.
    Dma,
    /// The firmware lacks what the driver relies on (a command version, a
    /// capability).
    Firmware(&'static str),
    /// The radio is switched off (the platform's kill switch).
    RfKill,
    /// The firmware did not start: no ALIVE in time, or not a good one
    /// (its status).
    NoAlive,
    Alive(u16),
    /// No answer in time: to a command, or a notification waited for.
    Timeout(&'static str),
    /// The firmware refused the command.
    Failed(CmdId),
    /// An answer the driver cannot read.
    Response(CmdId),
    /// The firmware or the hardware stopped; the firmware's error id from
    /// its log when it could be read.
    Fatal { hw: bool, error_id: Option<u32> },
    /// No network joined to send to, or too many frames on their way.
    NotJoined,
    QueueFull,
}

impl From<HwError> for Error {
    fn from(e: HwError) -> Self {
        Error::Hw(e)
    }
}

/// A notification from the firmware: its code (group << 8 | opcode, a
/// legacy one's group 0) and what follows the header.
#[derive(Debug, PartialEq, Eq)]
pub struct Notif {
    pub code: u16,
    pub data: Vec<u8>,
}

/// What the bring-up learned.
#[derive(Debug)]
pub struct Info {
    pub mac: [u8; 6],
    pub nvm: NvmInfo,
    /// The regulatory domain the firmware settled on, and the channels it
    /// allows ("ZZ" and the NVM's without location-aware regulatory).
    pub alpha2: [u8; 2],
    pub channels: Vec<Channel>,
    /// The antennas to send and receive with (the firmware's and the
    /// NVM's).
    pub tx_chains: u32,
    pub rx_chains: u32,
}

/// The access point being joined, as its beacons and the scan told.
#[derive(Clone, Copy, Debug)]
pub struct Peer {
    pub bssid: [u8; 6],
    pub channel: Channel,
    pub beacon: Beacon,
}

/// The station's transmit queue (DQA's first data queue): 256 descriptors
/// with the byte count table after them in one buffer, the frames (their
/// TX command and header, then the body) in another, [`TX_SLOTS`] of them
/// on their way at most.
struct TxQueue {
    ring: Dma,
    frames: Dma,
    /// The firmware's number for it (1), once added.
    qid: Option<u8>,
    cur: usize,
    /// The first index not yet done.
    tail: usize,
    failed: u32,
}

const TXQ_BYTE_COUNTS: usize = ring::CMD_RING * ring::TFD_LEN;
const TXQ_RING_LEN: usize = TXQ_BYTE_COUNTS + 4096;
const TX_SLOTS: usize = 64;
const TX_SLOT: usize = 4096;
/// Where a slot's body starts: after the TX command and header (at most
/// 4 + 20 + 32 + 3 bytes).
const TX_BODY: usize = 64;

/// What the firmware has of the network joined, so that leaving undoes
/// just that.
#[derive(Default)]
struct Joined {
    phy: bool,
    mac: bool,
    binding: bool,
    sta: bool,
    protected: bool,
    assoc: bool,
}

// The control buffer: the context information, the ICT, the receive
// ring's free and used descriptors and its status.
const CTXT: usize = 0;
const ICT: usize = 0x1000;
const ICT_COUNT: usize = 1024;
const FREE_RBD: usize = 0x2000;
const USED_RBD: usize = 0x3000;
const RX_STATUS: usize = 0x3800;
const CTL_LEN: usize = 0x4000;
/// The command buffer: the slots, then one for a bigger command.
const BOUNCE: usize = ring::CMD_RING * ring::CMD_SLOT;
const CMD_LEN: usize = (BOUNCE + 4096).next_multiple_of(4096);
/// Receive buffers per DMA buffer (4 of 512 KB, not one of 2 MB).
const RX_CHUNK: usize = 128;
/// The command queue (DQA's command queue 0).
const CMD_QID: u8 = 0;

const ALIVE_TIMEOUT_US: u64 = 1_000_000;
const CMD_TIMEOUT_US: u64 = 1_000_000;
const INIT_COMPLETE_TIMEOUT_US: u64 = 2_000_000;
/// The notifications kept for the caller at most (the frames of a scan).
const MAX_QUEUED: usize = 1024;

// The firmware's API and capabilities the driver relies on.
const API_WIFI_MCC_UPDATE: usize = 9;
const API_REDUCED_SCAN_CONFIG: usize = 56;
const API_SCAN_EXT_CHAN_VER: usize = 58;
const CAPA_LAR_MULTI_MCC: usize = 29;
const CAPA_CT_KILL_BY_FW: usize = 74;
const CAPA_MCC_UPDATE_11AX_SUPPORT: usize = 89;
const CAPA_UMAC_SCAN: usize = 2;
const SOC_LATENCY_US: u32 = 500;

/// Where each section goes in one DMA buffer: page-aligned offsets, and
/// the buffer's length.
fn pack(sections: &[Section]) -> (Vec<usize>, usize) {
    let mut off = 0;
    let offsets = sections
        .iter()
        .map(|s| {
            let at = off;
            off += s.data.len().next_multiple_of(4096);
            at
        })
        .collect();
    (offsets, off.max(4096))
}

/// The causes of the ICT's entries (iwx_intr): ORed, then spread back
/// into CSR_INT's layout.
fn ict_causes(mut r: u32) -> u32 {
    if r == 0xffff_ffff {
        r = 0;
    }
    if r & 0xc0000 != 0 {
        r |= 0x8000;
    }
    (r & 0xff) | ((r & 0xff00) << 16)
}

pub struct Dev<D: Device> {
    pub hw: Hw<D>,
    /// HW_REV as read, and in the driver's form (steps in bits 2-3).
    hw_rev_raw: u32,
    hw_rev: u32,
    ctl: Dma,
    tfd: Dma,
    cmds: Dma,
    rx: Vec<Dma>,
    /// The firmware's images (LMAC and UMAC, then paging): the device
    /// keeps reading the paging one.
    fw_mem: Vec<Dma>,
    use_ict: bool,
    ict_cur: usize,
    rx_cur: usize,
    cmd_cur: usize,
    /// The command waited for, and its answer {flags, data}.
    waiting: Option<u8>,
    answer: Option<(u8, Vec<u8>)>,
    queue: VecDeque<Notif>,
    fatal: Option<Error>,
    lmac_error_table: u32,
    buf: Vec<u8>,
    txq: Option<TxQueue>,
    joined: Joined,
}

impl<D: Device> Dev<D> {
    /// The device behind `dev`, its interrupt through MSI-X or MSI: an
    /// AX201 or nothing (iwx_attach). Its buffers are set up, nothing
    /// started.
    pub fn new(dev: D, msix: bool) -> Result<Self, Error> {
        let mut hw = Hw::new(dev, msix, true);
        hw.write(INT_MASK, 0);
        hw.write(INT, !0);
        hw.write(FH_INT_STATUS, !0);
        let raw = hw.read(HW_REV);
        let hw_rev = (raw & 0xfff0) | ((raw & 0x3) << 2);
        if hw_rev != HW_REV_TYPE_QUZ {
            return Err(Error::Unsupported(hw_rev));
        }
        let ctl = hw.bus.dma(CTL_LEN).ok_or(Error::Dma)?;
        let tfd = hw.bus.dma(ring::CMD_RING * ring::TFD_LEN).ok_or(Error::Dma)?;
        let cmds = hw.bus.dma(CMD_LEN).ok_or(Error::Dma)?;
        let rx = (0..ring::RX_RING / RX_CHUNK)
            .map(|_| hw.bus.dma(RX_CHUNK * ring::RX_BUF).ok_or(Error::Dma))
            .collect::<Result<Vec<_>, _>>()?;
        let mut d = Dev {
            hw,
            hw_rev_raw: raw,
            hw_rev,
            ctl,
            tfd,
            cmds,
            rx,
            fw_mem: Vec::new(),
            use_ict: false,
            ict_cur: 0,
            rx_cur: 0,
            cmd_cur: 0,
            waiting: None,
            answer: None,
            queue: VecDeque::new(),
            fatal: None,
            lmac_error_table: 0,
            buf: alloc::vec![0; ring::RX_BUF],
            txq: None,
            joined: Joined::default(),
        };
        d.stock_rx();
        Ok(d)
    }

    fn rx_phys(&self, i: usize) -> u64 {
        self.rx[i / RX_CHUNK].phys() + ((i % RX_CHUNK) * ring::RX_BUF) as u64
    }

    /// Every receive buffer free again (iwx_update_rx_desc).
    fn stock_rx(&mut self) {
        for i in 0..ring::RX_RING {
            let d = ring::rbd(self.rx_phys(i), i);
            self.ctl.write64(FREE_RBD + 8 * i, d);
        }
    }

    /// Start the hardware and the firmware, up to its ALIVE
    /// (iwx_start_hw, iwx_start_fw, iwx_load_firmware).
    pub fn load(&mut self, fw: &Firmware) -> Result<(), Error> {
        if !fw.capa(CAPA_UMAC_SCAN) || !fw.api(API_SCAN_EXT_CHAN_VER) || !fw.api(API_REDUCED_SCAN_CONFIG) {
            return Err(Error::Firmware("no UMAC scan with extended channels and reduced configuration"));
        }
        self.hw.start_hw()?;
        if self.hw.rfkill() {
            return Err(Error::RfKill);
        }
        self.place_firmware(fw)?;
        self.hw.nic_init(self.hw_rev, fw.phy_sku)?;
        self.hw.enable_fwload_interrupt();
        self.hw.kick_self_load(self.ctl.phys() + CTXT as u64)?;
        let alive = self
            .wait(ALIVE_TIMEOUT_US, |d| d.take(msg::ALIVE))
            .map_err(|e| if let Error::Timeout(_) = e { Error::NoAlive } else { e })?;
        let alive = msg::alive(&alive).ok_or(Error::NoAlive)?;
        if alive.status != msg::ALIVE_OK {
            return Err(Error::Alive(alive.status));
        }
        self.lmac_error_table = alive.lmac_error_table[0];
        // iwx_post_alive.
        for i in 0..ICT_COUNT {
            self.ctl.write32(ICT + 4 * i, 0);
        }
        self.ict_cur = 0;
        self.hw.ict_reset(self.ctl.phys() + ICT as u64);
        self.use_ict = true;
        Ok(())
    }

    /// The images into DMA memory and the context information pointing at
    /// them (iwx_init_fw_sec, iwx_ctxt_info_init).
    fn place_firmware(&mut self, fw: &Firmware) -> Result<(), Error> {
        let mut images: Vec<Section> = fw.lmac.clone();
        images.extend_from_slice(&fw.umac);
        self.fw_mem.clear();
        let mut addrs: [Vec<u64>; 2] = [Vec::new(), Vec::new()];
        for (k, sections) in [&images[..], &fw.paging[..]].into_iter().enumerate() {
            let (offsets, len) = pack(sections);
            let mut mem = self.hw.bus.dma(len).ok_or(Error::Dma)?;
            for (s, &off) in sections.iter().zip(&offsets) {
                mem.write(off, s.data);
                addrs[k].push(mem.phys() + off as u64);
            }
            self.fw_mem.push(mem);
        }
        let (lmac, umac) = addrs[0].split_at(fw.lmac.len());
        let a = CtxtAddrs {
            free_rbd: self.ctl.phys() + FREE_RBD as u64,
            used_rbd: self.ctl.phys() + USED_RBD as u64,
            rx_status: self.ctl.phys() + RX_STATUS as u64,
            cmd_queue: self.tfd.phys(),
            lmac,
            umac,
            paging: &addrs[1],
        };
        let mut ctxt = [0u8; ring::CTXT_INFO_LEN];
        ring::context_info(&mut ctxt, self.hw_rev_raw, &a).map_err(|_| Error::Dma)?;
        self.ctl.write(CTXT, &ctxt);
        Ok(())
    }

    /// After ALIVE: the NVM, the configuration commands, the regulatory
    /// domain, the scan configuration (iwx_run_init_mvm_ucode, iwx_init_hw).
    pub fn init(&mut self, fw: &Firmware) -> Result<Info, Error> {
        self.cmd(msg::INIT_EXTENDED_CFG, &msg::init_extended_cfg())?;
        self.cmd(msg::NVM_ACCESS_COMPLETE, &msg::nvm_access_complete())?;
        self.wait(INIT_COMPLETE_TIMEOUT_US, |d| d.take(msg::INIT_COMPLETE))?;
        let r = self.cmd(msg::NVM_GET_INFO, &msg::nvm_get_info())?;
        let nvm = msg::nvm_info(&r).ok_or(Error::Response(msg::NVM_GET_INFO))?;
        let mac = self.hw.mac_addr()?;

        let valid = |fw_chains: u32, nvm_chains: u32| if nvm_chains != 0 { fw_chains & nvm_chains } else { fw_chains };
        let tx = valid(fw.tx_chains(), nvm.tx_chains);
        let rx = valid(fw.rx_chains(), nvm.rx_chains);
        self.cmd(msg::TX_ANT_CONFIGURATION, &msg::tx_ant_configuration(tx))?;
        self.cmd(msg::BT_CONFIG, &msg::bt_config())?;
        self.cmd(msg::SOC_CONFIGURATION, &msg::soc_configuration(SOC_LATENCY_US))?;
        if fw.capa(CAPA_CT_KILL_BY_FW) {
            self.cmd(msg::TEMP_REPORTING_THRESHOLDS, &msg::temp_reporting_thresholds())?;
        }
        self.cmd(msg::POWER_TABLE, &msg::power_table())?;

        let (mut alpha2, mut channels) = (*b"ZZ", nvm.channels.clone());
        if nvm.lar_enabled {
            if !fw.capa(CAPA_MCC_UPDATE_11AX_SUPPORT) {
                return Err(Error::Firmware("no MCC_UPDATE response version 4"));
            }
            let current = fw.api(API_WIFI_MCC_UPDATE) || fw.capa(CAPA_LAR_MULTI_MCC);
            let r = self.cmd(msg::MCC_UPDATE, &msg::mcc_update(*b"ZZ", current))?;
            (alpha2, channels) = msg::mcc_response(&r, nvm.band_5ghz).ok_or(Error::Response(msg::MCC_UPDATE))?;
        }
        self.cmd(msg::SCAN_CFG, &msg::scan_cfg(tx, rx))?;
        Ok(Info { mac, nvm, alpha2, channels, tx_chains: tx, rx_chains: rx })
    }

    /// Get ready to authenticate with `peer`: the PHY context on its
    /// channel, the MAC context, their binding, the access point as the
    /// station with its queue, and the firmware kept on the channel while
    /// the daemon authenticates and associates (iwx_auth). Undone on an
    /// error.
    pub fn join(&mut self, info: &Info, peer: &Peer) -> Result<(), Error> {
        self.leave(info, peer);
        let r = self.join_steps(info, peer);
        if r.is_err() {
            self.leave(info, peer);
        }
        r
    }

    fn join_steps(&mut self, info: &Info, peer: &Peer) -> Result<(), Error> {
        self.cmd(sta::PHY_CONTEXT, &sta::phy_context(Action::Add, peer.channel))?;
        self.joined.phy = true;
        self.cmd(sta::RLC_CONFIG, &sta::rlc_config(info.rx_chains))?;
        self.cmd(sta::MAC_CONTEXT, &sta::mac_context(Action::Add, info.mac, peer.bssid, peer.channel.ghz5, None))?;
        self.joined.mac = true;
        self.cmd_status(sta::BINDING_CONTEXT, &sta::binding(Action::Add), |s| s == 0)?;
        self.joined.binding = true;
        self.cmd_status(sta::ADD_STA, &sta::add_sta(Some(peer.bssid)), |s| s & 0xff == 1)?;
        self.joined.sta = true;
        self.add_queue()?;
        let duration = sta::join_protection_tu(peer.beacon.interval);
        self.cmd(sta::SESSION_PROTECTION, &sta::session_protection(true, duration))?;
        self.joined.protected = true;
        Ok(())
    }

    /// The station's queue: its buffers the first time, then
    /// SCD_QUEUE_CONFIG adding it from index 0.
    fn add_queue(&mut self) -> Result<(), Error> {
        if self.txq.is_none() {
            let ring = self.hw.bus.dma(TXQ_RING_LEN).ok_or(Error::Dma)?;
            let frames = self.hw.bus.dma(TX_SLOTS * TX_SLOT).ok_or(Error::Dma)?;
            self.txq = Some(TxQueue { ring, frames, qid: None, cur: 0, tail: 0, failed: 0 });
        }
        let q = self.txq.as_mut().unwrap();
        q.ring.write(0, &alloc::vec![0; TXQ_RING_LEN]);
        let (tfds, bc) = (q.ring.phys(), q.ring.phys() + TXQ_BYTE_COUNTS as u64);
        let r = self.cmd(sta::SCD_QUEUE_CONFIG, &sta::queue_add(tfds, bc))?;
        let qid = sta::queue_added(&r).ok_or(Error::Response(sta::SCD_QUEUE_CONFIG))?;
        let q = self.txq.as_mut().unwrap();
        (q.qid, q.cur, q.tail) = (Some(qid as u8), 0, 0);
        Ok(())
    }

    /// Associated with `peer` as `aid`: the station and MAC context told,
    /// the smart FIFO on, group frames up, the device awake, the
    /// firmware's rate scaling started (iwx_run).
    pub fn associated(&mut self, info: &Info, peer: &Peer, aid: u16) -> Result<(), Error> {
        self.cmd_status(sta::ADD_STA, &sta::add_sta(None), |s| s & 0xff == 1)?;
        let assoc = Some(Assoc { aid, beacon: peer.beacon });
        self.cmd(sta::MAC_CONTEXT, &sta::mac_context(Action::Modify, info.mac, peer.bssid, peer.channel.ghz5, assoc))?;
        self.joined.assoc = true;
        self.cmd(sta::SF_CFG, &sta::sf_config(true))?;
        self.cmd(sta::MCAST_FILTER, &sta::mcast_filter(peer.bssid))?;
        self.cmd(sta::MAC_PM_POWER_TABLE, &sta::mac_power())?;
        // Its rates as the beacon said, the band's when it said none.
        let band: u16 = if peer.channel.ghz5 { 0xff0 } else { 0xfff };
        let rates = match peer.beacon.rates & band {
            0 => band,
            r => r,
        };
        self.cmd(sta::TLC_MNG_CONFIG, &sta::tlc_config(rates))?;
        Ok(())
    }

    /// Leave the network joined, as far as it was (iwx_run_stop, then
    /// iwx_deauth): what was sent dropped, the contexts removed. Errors
    /// are not stopped at: the next join starts clean anyway.
    pub fn leave(&mut self, info: &Info, peer: &Peer) {
        let j = core::mem::take(&mut self.joined);
        let queued = self.txq.as_ref().is_some_and(|q| q.qid.is_some());
        if j.sta {
            let _ = self.cmd_status(sta::ADD_STA, &sta::drain_sta(true), |s| s & 0xff == 1);
            let _ = self.cmd(sta::TXPATH_FLUSH, &sta::txpath_flush());
            let _ = self.cmd_status(sta::ADD_STA, &sta::drain_sta(false), |s| s & 0xff == 1);
        }
        if j.assoc {
            let _ = self.cmd(sta::SF_CFG, &sta::sf_config(false));
            let mac = sta::mac_context(Action::Modify, info.mac, peer.bssid, peer.channel.ghz5, None);
            let _ = self.cmd(sta::MAC_CONTEXT, &mac);
        }
        if j.protected {
            let _ = self.cmd(sta::SESSION_PROTECTION, &sta::session_protection(false, 0));
        }
        if queued {
            let _ = self.cmd(sta::SCD_QUEUE_CONFIG, &sta::queue_remove());
            if let Some(q) = self.txq.as_mut() {
                q.qid = None;
            }
        }
        if j.sta {
            let _ = self.cmd(sta::REMOVE_STA, &sta::remove_sta());
        }
        if j.binding {
            let _ = self.cmd_status(sta::BINDING_CONTEXT, &sta::binding(Action::Remove), |s| s == 0);
        }
        if j.mac {
            let _ = self.cmd(sta::MAC_CONTEXT, &sta::mac_context(Action::Remove, info.mac, peer.bssid, false, None));
        }
        if j.phy {
            let _ = self.cmd(sta::PHY_CONTEXT, &sta::phy_context(Action::Remove, peer.channel));
        }
    }

    /// Send 802.11 frame `f` (already encrypted, when it is) on the
    /// station's queue: lowest rate for management and group frames,
    /// `ofdm` on 5 GHz (iwx_tx).
    pub fn send(&mut self, f: &[u8], ofdm: bool) -> Result<(), Error> {
        if let Some(e) = self.fatal {
            return Err(e);
        }
        let q = self.txq.as_mut().ok_or(Error::NotJoined)?;
        let qid = q.qid.ok_or(Error::NotJoined)?;
        if (q.cur + ring::CMD_RING - q.tail) % ring::CMD_RING >= TX_SLOTS - 1 {
            return Err(Error::QueueFull);
        }
        let idx = q.cur;
        let slot = (idx % TX_SLOTS) * TX_SLOT;
        let mut c = [0u8; TX_BODY];
        let n = sta::tx_cmd(&mut c, qid, idx as u8, f, ofdm).ok_or(Error::Response(CmdId::legacy(sta::TX_CMD)))?;
        let hdr = msg::header_len(f).min(f.len());
        let body = &f[hdr..];
        if TX_BODY + body.len() > TX_SLOT {
            return Err(Error::Response(CmdId::legacy(sta::TX_CMD)));
        }
        q.frames.write(slot, &c[..n]);
        q.frames.write(slot + TX_BODY, body);
        let at = q.frames.phys() + slot as u64;
        let mut tbs = alloc::vec![(at, 20), (at + 20, n - 20)];
        if !body.is_empty() {
            tbs.push((at + TX_BODY as u64, body.len()));
        }
        let mut tfd = [0u8; ring::TFD_LEN];
        ring::encode_tfd_tbs(&mut tfd, &tbs);
        q.ring.write(idx * ring::TFD_LEN, &tfd);
        q.ring.write(TXQ_BYTE_COUNTS + 2 * idx, &sta::byte_count(f.len(), tbs.len()).to_le_bytes());
        q.cur = (idx + 1) % ring::CMD_RING;
        let cur = q.cur as u32;
        self.hw.write(HBUS_TARG_WRPTR, u32::from(qid) << 16 | cur);
        Ok(())
    }

    /// Wait up to `timeout_us` for the frames sent to be done (a
    /// deauthentication, before leaving flushes the queue).
    pub fn drain(&mut self, timeout_us: u64) {
        let done = |d: &mut Self| d.txq.as_ref().is_none_or(|q| q.qid.is_none() || q.tail == q.cur).then_some(());
        let _ = self.wait(timeout_us, done);
    }

    /// Frames the firmware could not deliver (no acknowledgement) so far.
    pub fn tx_failed(&self) -> u32 {
        self.txq.as_ref().map_or(0, |q| q.failed)
    }

    /// A command answered with a status word that `ok` accepts
    /// (iwx_send_cmd_pdu_status).
    fn cmd_status(&mut self, id: CmdId, payload: &[u8], ok: impl Fn(u32) -> bool) -> Result<(), Error> {
        let r = self.cmd(id, payload)?;
        if r.len() < 4 {
            return Err(Error::Response(id));
        }
        if ok(ring::le32(&r, 0)) { Ok(()) } else { Err(Error::Failed(id)) }
    }

    /// Start a passive scan of `channels`; its frames and its end come as
    /// notifications (REPLY_RX_MPDU, then SCAN_COMPLETE_UMAC).
    pub fn scan(&mut self, fw: &Firmware, info: &Info) -> Result<(), Error> {
        if fw.cmd_ver(ring::LONG_GROUP, msg::SCAN_REQ_UMAC.opcode) == 17 {
            return Err(Error::Firmware("SCAN_REQ_UMAC version 17"));
        }
        let c = msg::scan_v14(&info.channels, fw.n_scan_channels as usize, info.mac, info.nvm.band_5ghz);
        self.cmd(msg::SCAN_REQ_UMAC, &c)?;
        Ok(())
    }

    /// The next notification, waiting up to `timeout_us`; `None` when none
    /// came.
    pub fn next(&mut self, timeout_us: u64) -> Result<Option<Notif>, Error> {
        match self.wait(timeout_us, |d| d.queue.pop_front()) {
            Ok(n) => Ok(Some(n)),
            Err(Error::Timeout(_)) => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// The next notification without waiting: an interrupt that came is
    /// serviced first.
    pub fn poll(&mut self) -> Option<Notif> {
        if self.queue.is_empty() && self.fatal.is_none() && self.hw.bus.wait_irq(0) {
            self.service();
        }
        self.queue.pop_front()
    }

    /// Whether notifications are waiting (the interrupt that brought them
    /// already serviced).
    pub fn pending(&self) -> bool {
        !self.queue.is_empty()
    }

    /// The first queued notification `code`, the others left.
    fn take(&mut self, code: u16) -> Option<Vec<u8>> {
        let i = self.queue.iter().position(|n| n.code == code)?;
        self.queue.remove(i).map(|n| n.data)
    }

    /// Wait for `done` to give something, servicing the interrupt.
    fn wait<T>(&mut self, timeout_us: u64, mut done: impl FnMut(&mut Self) -> Option<T>) -> Result<T, Error> {
        let deadline = self.hw.bus.now_us().saturating_add(timeout_us);
        loop {
            if let Some(e) = self.fatal {
                return Err(e);
            }
            if let Some(t) = done(self) {
                return Ok(t);
            }
            let now = self.hw.bus.now_us();
            if now >= deadline {
                return Err(Error::Timeout("firmware"));
            }
            if self.hw.bus.wait_irq(deadline - now) {
                self.service();
            }
        }
    }

    /// Send a command and wait for its answer (iwx_send_cmd, synchronous).
    pub fn cmd(&mut self, id: CmdId, payload: &[u8]) -> Result<Vec<u8>, Error> {
        if let Some(e) = self.fatal {
            return Err(e);
        }
        let idx = self.cmd_cur;
        let mut c = [0u8; ring::CMD_HDR + ring::CMD_MAX_PAYLOAD];
        let len = ring::encode_cmd(&mut c, id, CMD_QID, idx as u8, payload).ok_or(Error::Response(id))?;
        let at = if payload.len() > ring::CMD_SLOT_DATA { BOUNCE } else { idx * ring::CMD_SLOT };
        self.cmds.write(at, &c[..len]);
        let mut tfd = [0u8; ring::TFD_LEN];
        ring::encode_tfd(&mut tfd, self.cmds.phys() + at as u64, len);
        self.tfd.write(idx * ring::TFD_LEN, &tfd);
        self.cmd_cur = (idx + 1) % ring::CMD_RING;
        self.waiting = Some(idx as u8);
        self.answer = None;
        self.hw.write(HBUS_TARG_WRPTR, (CMD_QID as u32) << 16 | self.cmd_cur as u32);
        let r = self.wait(CMD_TIMEOUT_US, |d| d.answer.take());
        self.waiting = None;
        let (flags, data) = r.map_err(|e| if let Error::Timeout(_) = e { Error::Timeout("command") } else { e })?;
        if flags & msg::CMD_FAILED != 0 {
            return Err(Error::Failed(id));
        }
        Ok(data)
    }

    /// The interrupt: its causes, then what came (iwx_intr, iwx_intr_msix;
    /// an INTx line as MSI).
    pub fn service(&mut self) {
        if self.hw.msix {
            self.service_msix();
        } else {
            self.service_msi();
        }
        self.hw.bus.irq_done();
    }

    fn service_msi(&mut self) {
        self.hw.write(INT_MASK, 0);
        let (r1, r2) = if self.use_ict {
            let mut r = 0;
            loop {
                let e = self.ctl.read32(ICT + 4 * self.ict_cur);
                if e == 0 {
                    break;
                }
                r |= e;
                self.ctl.write32(ICT + 4 * self.ict_cur, 0);
                self.ict_cur = (self.ict_cur + 1) % ICT_COUNT;
            }
            (ict_causes(r), 0)
        } else {
            let r1 = self.hw.read(INT);
            if r1 == 0xffff_ffff || r1 & 0xffff_fff0 == 0xa5a5_a5a0 {
                return; // the device is gone (the mask stays clear)
            }
            (r1, self.hw.read(FH_INT_STATUS))
        };
        if r1 != 0 || r2 != 0 {
            self.hw.write(INT, r1 | !self.hw.intmask);
            if r1 & INT_ALIVE != 0 {
                self.restock_on_alive();
            }
            if r1 & INT_SW_ERR != 0 {
                self.fail(false);
                return;
            }
            if r1 & INT_HW_ERR != 0 {
                self.fail(true);
                return;
            }
            if r1 & INT_RF_KILL != 0 && self.hw.rfkill() {
                self.fatal = Some(Error::RfKill);
            }
            if r1 & (INT_FH_RX | INT_SW_RX | INT_RX_PERIODIC) != 0 {
                if r1 & (INT_FH_RX | INT_SW_RX) != 0 {
                    self.hw.write(FH_INT_STATUS, FH_INT_RX);
                }
                if r1 & INT_RX_PERIODIC != 0 {
                    self.hw.write(INT, INT_RX_PERIODIC);
                }
                // The periodic interrupt as a one-shot, 8 ms after a real
                // one, for a straggling frame.
                self.hw.bus.write8(INT_PERIODIC, INT_PERIODIC_DIS);
                if r1 & (INT_FH_RX | INT_SW_RX) != 0 {
                    self.hw.bus.write8(INT_PERIODIC, INT_PERIODIC_ENA);
                }
                self.rx();
            }
        }
        self.hw.write(INT_MASK, self.hw.intmask);
    }

    fn service_msix(&mut self) {
        let fh = self.hw.read(MSIX_FH_INT_CAUSES);
        let hwc = self.hw.read(MSIX_HW_INT_CAUSES);
        self.hw.write(MSIX_FH_INT_CAUSES, fh);
        self.hw.write(MSIX_HW_INT_CAUSES, hwc);
        let (fh, hwc) = (fh & self.hw.fh_mask, hwc & self.hw.hw_mask);
        if fh & (MSIX_FH_Q0 | MSIX_FH_Q1) != 0 {
            self.rx();
        }
        if hwc & (MSIX_HW_TOP_FATAL_ERR | MSIX_HW_HW_ERR) != 0 {
            self.fail(true);
            return;
        }
        if fh & MSIX_FH_ERR != 0 || hwc & (MSIX_HW_SW_ERR | MSIX_HW_SW_ERR_V2) != 0 {
            self.fail(false);
            return;
        }
        if hwc & MSIX_HW_RF_KILL != 0 && self.hw.rfkill() {
            self.fatal = Some(Error::RfKill);
        }
        if hwc & MSIX_HW_ALIVE != 0 {
            self.restock_on_alive();
        }
        // The cause masked itself; unmask it (write 1 to clear).
        self.hw.write(MSIX_AUTOMASK_ST, 1);
    }

    /// The firmware has set the receive queue up: every buffer to it.
    fn restock_on_alive(&mut self) {
        self.stock_rx();
        self.hw.write(RFH_Q0_FRBDCB_WIDX_TRG, 8);
    }

    /// The firmware or hardware stopped (the interrupt stays masked): read
    /// the firmware's error id when its log is known (iwx_nic_error).
    fn fail(&mut self, hw: bool) {
        let mut words = [0u32; 2];
        let error_id = (self.lmac_error_table != 0 && self.hw.read_mem(self.lmac_error_table, &mut words).is_ok())
            .then_some(words[1]);
        self.fatal = Some(Error::Fatal { hw, error_id });
    }

    /// The buffers the device closed (iwx_notif_intr).
    fn rx(&mut self) {
        let mut status = [0u8; 2];
        self.ctl.read(RX_STATUS, &mut status);
        let hw = (u16::from_le_bytes(status) as usize & 0xfff) % ring::RX_RING;
        let mut buf = core::mem::take(&mut self.buf);
        while self.rx_cur != hw {
            let i = self.rx_cur;
            self.rx[i / RX_CHUNK].read((i % RX_CHUNK) * ring::RX_BUF, &mut buf);
            for p in ring::packets(&buf) {
                self.packet(p);
            }
            self.rx_cur = (i + 1) % ring::RX_RING;
        }
        self.buf = buf;
        // Hand them back, the index a multiple of 8.
        let last = if hw == 0 { ring::RX_RING - 1 } else { hw - 1 };
        self.hw.write(RFH_Q0_FRBDCB_WIDX_TRG, (last & !7) as u32);
    }

    fn packet(&mut self, p: Packet) {
        // A legacy command went as the long group, and so does its answer.
        let code = if p.group() == ring::LONG_GROUP { p.opcode() as u16 } else { p.code };
        if !p.is_notification() {
            if p.qid == CMD_QID && self.waiting == Some(p.idx) {
                self.answer = Some((p.flags, p.data.to_vec()));
            }
            // A frame sent: the ones before the index it names are done
            // (iwx_rx_tx_cmd).
            if let Some(q) = self.txq.as_mut().filter(|q| q.qid == Some(p.qid) && code == u16::from(sta::TX_CMD)) {
                if let Some((ok, ssn)) = sta::tx_done(p.data) {
                    q.failed += u32::from(!ok);
                    if (ssn as usize) < ring::CMD_RING {
                        q.tail = ssn as usize;
                    }
                }
            }
            return;
        }
        if self.queue.len() < MAX_QUEUED {
            self.queue.push_back(Notif { code, data: p.data.to_vec() });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ring::{put16, put32};
    use alloc::boxed::Box;
    use alloc::collections::BTreeMap;
    use alloc::vec;
    use core::cell::RefCell;
    use std::rc::Rc;

    extern crate std;

    /// Registers that read back what was written (and HW_REV a QuZ), DMA
    /// from the heap at made-up addresses, an interrupt that runs `irq`
    /// on the buffers (in the order asked for).
    struct Fake {
        regs: BTreeMap<u32, u32>,
        writes: Vec<(u32, u32)>,
        bufs: Rc<RefCell<Vec<(*mut u8, usize)>>>,
        irq: Option<Box<dyn FnMut(&mut [(*mut u8, usize)], &[(u32, u32)]) -> bool>>,
        now: u64,
    }

    impl Bus for Fake {
        fn read32(&mut self, off: u32) -> u32 {
            *self.regs.get(&off).unwrap_or(&0)
        }
        fn write32(&mut self, off: u32, val: u32) {
            self.writes.push((off, val));
            self.regs.insert(off, val);
        }
        fn write8(&mut self, off: u32, val: u8) {
            self.writes.push((off, val as u32));
        }
        fn delay_us(&mut self, us: u32) {
            self.now += us as u64;
        }
    }

    impl Device for Fake {
        fn dma(&mut self, len: usize) -> Option<Dma> {
            let mem: &'static mut [u8] = Box::leak(vec![0u8; len + 4096].into_boxed_slice());
            let ptr = mem.as_mut_ptr().wrapping_add(mem.as_ptr().align_offset(4096));
            let mut bufs = self.bufs.borrow_mut();
            let phys = 0x1000_0000 * (bufs.len() as u64 + 1);
            bufs.push((ptr, len));
            // SAFETY: leaked, so it lives on; the test touches it only
            // through the fake interrupt.
            Some(unsafe { Dma::new(ptr, len, phys) })
        }
        fn wait_irq(&mut self, timeout_us: u64) -> bool {
            let fired = match self.irq.as_mut() {
                Some(f) => f(&mut self.bufs.borrow_mut(), &self.writes),
                None => false,
            };
            if fired {
                self.regs.insert(INT, INT_FH_RX);
            } else {
                self.now += timeout_us;
            }
            fired
        }
        fn now_us(&mut self) -> u64 {
            self.now
        }
    }

    fn fake() -> Fake {
        let mut regs = BTreeMap::new();
        regs.insert(HW_REV, 0x351);
        Fake { regs, writes: Vec::new(), bufs: Rc::new(RefCell::new(Vec::new())), irq: None, now: 0 }
    }

    /// Put `packets` {code, flags, idx, qid, data} in receive buffer `i`
    /// and close it.
    fn deliver(bufs: &mut [(*mut u8, usize)], i: usize, packets: &[(u8, u8, u8, u8, &[u8])]) {
        let (rx, _) = bufs[3 + i / RX_CHUNK];
        // SAFETY: the fake's leaked buffers.
        let b = unsafe { core::slice::from_raw_parts_mut(rx.add((i % RX_CHUNK) * 4096), 4096) };
        let mut off = 0;
        for (code, flags, idx, qid, data) in packets {
            put32(b, off, (4 + data.len()) as u32);
            b[off + 4..off + 8].copy_from_slice(&[*code, *flags, *idx, *qid]);
            b[off + 8..off + 8 + data.len()].copy_from_slice(data);
            off += (8 + data.len()).next_multiple_of(64);
        }
        let (ctl, _) = bufs[0];
        let s = unsafe { core::slice::from_raw_parts_mut(ctl.add(RX_STATUS), 2) };
        put16(s, 0, (i + 1) as u16);
    }

    #[test]
    fn not_an_ax201() {
        let mut f = fake();
        f.regs.insert(HW_REV, 0x340);
        assert!(matches!(Dev::new(f, false), Err(Error::Unsupported(0x340))));
    }

    #[test]
    fn buffers_and_ring() {
        let d = Dev::new(fake(), false).unwrap();
        // control, TFDs, commands, 4 receive buffers.
        assert_eq!(d.hw.bus.bufs.borrow().len(), 7);
        let (ctl, _) = d.hw.bus.bufs.borrow()[0];
        let free = unsafe { core::slice::from_raw_parts(ctl.add(FREE_RBD), 512 * 8) };
        assert_eq!(ring::le64(free, 0), 0x4000_0000);
        assert_eq!(ring::le64(free, 8 * 129), 0x5000_0000 + 4096 | 129);
        assert_eq!(CMD_LEN, 22 * 4096);
    }

    #[test]
    fn command_answer_and_notifications() {
        let mut f = fake();
        f.irq = Some(Box::new(|bufs, writes| {
            // The command at index 0 kicked: answer it, after a notification.
            if writes.last() != Some(&(HBUS_TARG_WRPTR, 1)) {
                return false;
            }
            deliver(bufs, 0, &[(0x04, 0, 0, 0x80, &[]), (0x98, 1, 0, 0, &[7, 0, 0, 0])]);
            true
        }));
        let mut d = Dev::new(f, false).unwrap();
        d.hw.intmask = INT_INI_SET;
        let r = d.cmd(msg::TX_ANT_CONFIGURATION, &msg::tx_ant_configuration(3)).unwrap();
        assert_eq!(r, [7, 0, 0, 0]);
        // The command in slot 0, its descriptor in TFD 0.
        let bufs = d.hw.bus.bufs.borrow().clone();
        let cmd = unsafe { core::slice::from_raw_parts(bufs[2].0, 12) };
        assert_eq!(cmd, &[0x98, 1, 0, 0, 4, 0, 0, 0, 3, 0, 0, 0]);
        let tfd = unsafe { core::slice::from_raw_parts(bufs[1].0, 14) };
        assert_eq!((ring::le16(tfd, 0), ring::le16(tfd, 2), ring::le64(tfd, 4)), (1, 12, 0x3000_0000));
        // The buffer given back, the notification kept.
        assert!(d.hw.bus.writes.contains(&(RFH_Q0_FRBDCB_WIDX_TRG, 0)));
        assert_eq!(d.take(msg::INIT_COMPLETE), Some(vec![]));
        // A refused command, then no answer at all.
        drop(bufs);
        d.hw.bus.irq = Some(Box::new(|bufs, writes| {
            if writes.last() != Some(&(HBUS_TARG_WRPTR, 2)) {
                return false;
            }
            deliver(bufs, 1, &[(0x9b, 1 | msg::CMD_FAILED, 1, 0, &[])]);
            true
        }));
        assert_eq!(d.cmd(msg::BT_CONFIG, &msg::bt_config()), Err(Error::Failed(msg::BT_CONFIG)));
        assert_eq!(d.cmd(msg::BT_CONFIG, &msg::bt_config()), Err(Error::Timeout("command")));
    }

    #[test]
    fn ict() {
        assert_eq!(ict_causes(0xffff_ffff), 0);
        assert_eq!(ict_causes(0x80), INT_RF_KILL);
        // Bit 15 is FH_RX (31), set too by the RX DMA bits 18-19.
        assert_eq!(ict_causes(0x40000), INT_FH_RX);
        assert_eq!(ict_causes(0x0201), INT_SW_ERR | INT_ALIVE);
    }

    /// With IWX_FIRMWARE naming the pinned file: its sections into two
    /// buffers, each named in the context information.
    #[test]
    fn pinned_firmware_placed() {
        let Ok(path) = std::env::var("IWX_FIRMWARE") else {
            return;
        };
        let file = std::fs::read(path).unwrap();
        let fw = Firmware::parse(&file).unwrap();
        let mut d = Dev::new(fake(), false).unwrap();
        d.place_firmware(&fw).unwrap();
        let bufs = d.hw.bus.bufs.borrow().clone();
        assert_eq!(bufs.len(), 9);
        let ctxt = unsafe { core::slice::from_raw_parts(bufs[0].0, ring::CTXT_INFO_LEN) };
        let entry = |image: usize, i: usize| ring::le64(ctxt, 192 + 8 * (64 * image + i));
        // UMAC first in the map, after the LMAC sections in the buffer.
        assert_eq!(entry(1, 0), 0x8000_0000);
        assert_ne!(entry(0, fw.umac.len() - 1), 0);
        assert_eq!(entry(0, fw.umac.len()), 0);
        assert_eq!(entry(2, 0), 0x9000_0000);
        assert_ne!(entry(2, fw.paging.len() - 1), 0);
        let first = unsafe { core::slice::from_raw_parts(bufs[7].0, 16) };
        assert_eq!(first, &fw.lmac[0].data[..16]);
    }

    /// A firmware that answers every command (ADD_STA and the binding
    /// with success, SCD_QUEUE_CONFIG with queue 1) and every frame sent
    /// (done, the first `failing` of them not acknowledged), noting the
    /// commands' {group, opcode} in `log`.
    fn answering(log: Rc<RefCell<Vec<(u8, u8)>>>, failing: usize) -> Fake {
        let mut f = fake();
        let mut rx = 0;
        let mut kicks = 0;
        let mut sent = 0;
        f.irq = Some(Box::new(move |bufs, writes| {
            // The queue kicks not answered yet: the next one.
            let Some(&(_, w)) = writes.iter().filter(|(r, _)| *r == HBUS_TARG_WRPTR).nth(kicks) else {
                return false;
            };
            kicks += 1;
            let (qid, cur) = ((w >> 16) as u8, (w & 0xff) as usize);
            let idx = (cur + ring::CMD_RING - 1) % ring::CMD_RING;
            let mut answer = vec![0u8; 0];
            let (code, group);
            if qid == 0 {
                // SAFETY: the fake's leaked buffers.
                let c = unsafe { core::slice::from_raw_parts(bufs[2].0.add(idx * ring::CMD_SLOT), 8) };
                (code, group) = (c[0], c[1]);
                log.borrow_mut().push((group, code));
                answer = match (group, code) {
                    (1, 0x18) => vec![1, 0, 0, 0],
                    (1, 0x2b) => vec![0; 4],
                    (5, 0x17) => vec![1, 0, 0, 0, 0, 0, 0, 0],
                    _ => vec![],
                };
            } else {
                (code, group) = (sta::TX_CMD, 0);
                answer = vec![0u8; 48];
                answer[0] = 1;
                put16(&mut answer, 40, if sent < failing { 0x83 } else { 1 });
                put32(&mut answer, 44, cur as u32);
                sent += 1;
            }
            deliver(bufs, rx, &[(code, group, idx as u8, qid, &answer)]);
            rx = (rx + 1) % ring::RX_RING;
            true
        }));
        f
    }

    fn peer() -> Peer {
        let beacon = Beacon { tsf: 1, gp2: 2, interval: 100, dtim_period: 1, dtim_count: 0, rates: 0x15f };
        Peer { bssid: [2, 0, 0, 0, 0, 1], channel: Channel { number: 6, ghz5: false, passive: false }, beacon }
    }

    fn info() -> Info {
        let nvm = msg::NvmInfo {
            nvm_version: 0,
            band_24ghz: true,
            band_5ghz: true,
            tx_chains: 3,
            rx_chains: 3,
            lar_enabled: true,
            channels: vec![],
        };
        Info { mac: [2, 0, 0, 0, 0, 9], nvm, alpha2: *b"ZZ", channels: vec![], tx_chains: 3, rx_chains: 3 }
    }

    #[test]
    fn join_send_leave() {
        let log = Rc::new(RefCell::new(Vec::new()));
        let mut d = Dev::new(answering(log.clone(), 1), false).unwrap();
        d.hw.intmask = INT_INI_SET;
        let (info, peer) = (info(), peer());
        assert_eq!(d.send(&[0x08; 30], false), Err(Error::NotJoined));
        d.join(&info, &peer).unwrap();
        // PHY, RLC, MAC, binding, station, queue, session protection.
        assert_eq!(*log.borrow(), [(1, 0x08), (5, 0x08), (1, 0x28), (1, 0x2b), (1, 0x18), (5, 0x17), (3, 0x05)]);
        // Control, TFDs, commands, 4 receive buffers, then the queue's 2.
        assert_eq!(d.hw.bus.bufs.borrow().len(), 9);

        // An authentication frame: three transfer buffers, its byte count,
        // the queue's write pointer; its completion frees the slot.
        let mut auth = vec![0xb0, 0, 0, 0, 2, 0, 0, 0, 0, 1];
        auth.resize(30, 0);
        d.send(&auth, false).unwrap();
        assert_eq!(d.hw.bus.writes.last(), Some(&(HBUS_TARG_WRPTR, 1 << 16 | 1)));
        let bufs = d.hw.bus.bufs.borrow().clone();
        let tfd = unsafe { core::slice::from_raw_parts(bufs[7].0, 40) };
        assert_eq!((ring::le16(tfd, 0), ring::le16(tfd, 2), ring::le16(tfd, 12), ring::le16(tfd, 22)), (3, 20, 28, 6));
        assert_eq!(ring::le64(tfd, 24), 0x9000_0000 + TX_BODY as u64);
        let bc = unsafe { core::slice::from_raw_parts(bufs[7].0.add(TXQ_BYTE_COUNTS), 2) };
        assert_eq!(ring::le16(bc, 0), 8);
        let body = unsafe { core::slice::from_raw_parts(bufs[8].0.add(TX_BODY), 6) };
        assert_eq!(body, &auth[24..]);
        d.drain(1000);
        let q = d.txq.as_ref().unwrap();
        assert_eq!((q.cur, q.tail, d.tx_failed()), (1, 1, 1));

        log.borrow_mut().clear();
        d.associated(&info, &peer, 1).unwrap();
        assert_eq!(*log.borrow(), [(1, 0x18), (1, 0x28), (1, 0xd1), (1, 0xd0), (1, 0xa9), (5, 0x0f)]);
        log.borrow_mut().clear();
        d.leave(&info, &peer);
        assert_eq!(
            *log.borrow(),
            [
                (1, 0x18), (1, 0x1e), (1, 0x18), // drain, flush, undrain
                (1, 0xd1), (1, 0x28), (3, 0x05), // SF off, not associated, protection off
                (5, 0x17), (1, 0x19), (1, 0x2b), (1, 0x28), (1, 0x08),
            ]
        );
        assert_eq!(d.send(&auth, false), Err(Error::NotJoined));
        // Nothing joined: leaving again sends nothing.
        log.borrow_mut().clear();
        d.leave(&info, &peer);
        assert!(log.borrow().is_empty());
    }

    #[test]
    fn firmware_packing() {
        let data = [0u8; 5000];
        let s = [Section { devoff: 1, data: &data[..5000] }, Section { devoff: 2, data: &data[..10] }];
        assert_eq!(pack(&s), (vec![0, 8192], 12288));
        assert_eq!(pack(&[]), (vec![], 4096));
    }
}
