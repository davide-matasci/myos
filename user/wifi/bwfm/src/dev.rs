//! The FullMAC driver over the bus (ported from bwfm.c's station side):
//! the firmware brought up and configured, its scans, a network joined
//! and left (the firmware authenticates, associates and, with its
//! supplicant, runs WPA2-PSK), Ethernet frames to and from it, and its
//! events. The daemon drives it: `service` when the card's interrupt
//! fires or while something is awaited, the queues in between.

use alloc::collections::VecDeque;
use alloc::vec;
use alloc::vec::Vec;

use crate::Error;
use crate::bcdc::{self, Dcmd};
use crate::bus::{Bus, Channel};
use crate::chip::Known;
use crate::event::{self, Scanned};
use crate::fw;
use crate::reg::*;
use crate::sdio::Sdio;

/// How long the firmware may take to answer a command.
pub const CONTROL_TIMEOUT_MS: u64 = 1000;

/// The link to the network being joined, or joined.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Link {
    Down,
    Joining,
    Up,
    /// The firmware gave up: its status.
    Failed(u32),
}

/// What the firmware says of itself once up.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Info {
    pub chip: u32,
    pub chiprev: u32,
    pub io_type: u32,
    pub mac: [u8; 6],
    pub scan_ver: u32,
}

pub struct Dev<S: Sdio> {
    pub bus: Bus<S>,
    reqid: u16,
    pub io_type: u32,
    pub mac: [u8; 6],
    scan_ver: u32,
    /// Control responses, events and Ethernet frames the card sent and
    /// nobody took yet.
    ctl: VecDeque<Vec<u8>>,
    events: VecDeque<Vec<u8>>,
    rx: VecDeque<Vec<u8>>,
    /// A scan under way (its results so far), and the last one's results.
    scan: Option<Vec<Scanned>>,
    scanned: Option<Vec<Scanned>>,
    link: Link,
    bssid: Option<[u8; 6]>,
    /// Joining with the firmware's supplicant: the link is up once keyed.
    psk: bool,
    link_up: bool,
    keyed: bool,
}

/// Ethernet frames kept for the host at most.
const RX_QUEUE: usize = 64;

impl<S: Sdio> Dev<S> {
    /// Attach to the card: the chip's cores read, its CPU held.
    pub fn attach(sdio: S) -> Result<Dev<S>, Error> {
        let bus = Bus::attach(sdio)?;
        Ok(Dev {
            bus,
            reqid: 1,
            io_type: 0,
            mac: [0; 6],
            scan_ver: 0,
            ctl: VecDeque::new(),
            events: VecDeque::new(),
            rx: VecDeque::new(),
            scan: None,
            scanned: None,
            link: Link::Down,
            bssid: None,
            psk: false,
            link_up: false,
            keyed: false,
        })
    }

    /// The chip, if the driver knows it (its firmware's names).
    pub fn known(&self) -> Option<&'static Known> {
        Known::by_id(self.bus.chip.id)
    }

    /// The firmware and the board's NVRAM (its `.txt`) into the chip, the
    /// CPU started, the firmware asked what it is and set up for a
    /// station (`clm`: the regulatory blob, where there is one).
    pub fn start(&mut self, firmware: &[u8], nvram_text: &[u8], clm: Option<&[u8]>) -> Result<Info, Error> {
        let nvram = fw::nvram(nvram_text, None);
        self.bus.load_firmware(firmware, &nvram)?;
        self.bus.start()?;
        self.io_type = self.cmd_get_int(C_GET_VERSION)?;
        let mac = self.var_get("cur_etheraddr", 6)?;
        self.mac.copy_from_slice(&mac[..6]);
        if let Some(clm) = clm {
            for piece in fw::dload(clm) {
                match self.var_set("clmload", &piece) {
                    Ok(()) | Err(Error::Status(_)) => {}
                    Err(e) => return Err(e),
                }
            }
        }
        self.scan_ver = self.var_get_int("scan_ver").unwrap_or(0);
        self.init()?;
        Ok(Info { chip: self.bus.chip.id, chiprev: self.bus.chip.rev, io_type: self.io_type, mac: self.mac, scan_ver: self.scan_ver })
    }

    /// The firmware as a station: the events wanted, the scan times, power
    /// saving, up, infrastructure mode, no offloading, its own supplicant.
    fn init(&mut self) -> Result<(), Error> {
        self.var_set_int("mpc", 1)?;
        // Join by RSSI, 5 GHz boosted.
        self.var_set("join_pref", &[4, 2, 8, 1, 1, 2, 0, 0])?;
        let mask = event::mask(&[
            E_SET_SSID,
            E_AUTH,
            E_DEAUTH,
            E_DEAUTH_IND,
            E_ASSOC,
            E_DISASSOC,
            E_DISASSOC_IND,
            E_LINK,
            E_EAPOL_MSG,
            E_PSK_SUP,
            E_IF,
            E_ESCAN_RESULT,
        ]);
        self.var_set("event_msgs", &mask)?;
        self.cmd_set_int(C_SET_SCAN_CHANNEL_TIME, DEFAULT_SCAN_CHANNEL_TIME)?;
        self.cmd_set_int(C_SET_SCAN_UNASSOC_TIME, DEFAULT_SCAN_UNASSOC_TIME)?;
        self.cmd_set_int(C_SET_SCAN_PASSIVE_TIME, DEFAULT_SCAN_PASSIVE_TIME)?;
        self.cmd_set_int(C_SET_PM, PM_FAST_PS)?;
        let _ = self.var_set_int("txbf", 1);
        self.cmd_set_int(C_UP, 0)?;
        self.cmd_set_int(C_SET_INFRA, 1)?;
        self.cmd_set_int(C_SET_AP, 0)?;
        for v in ["arp_ol", "arpoe", "ndoe", "toe"] {
            let _ = self.var_set_int(v, 0);
        }
        // The firmware's supplicant runs the 4-way handshake.
        self.var_set_int("sup_wpa", 1)?;
        Ok(())
    }

    // ---- Commands and variables -------------------------------------------

    /// One control transaction: `cmd` with `data`, a `set` or a query; the
    /// firmware's answer (its header and data).
    pub fn control(&mut self, cmd: u32, set: bool, data: &[u8]) -> Result<(Dcmd, Vec<u8>), Error> {
        if data.len() > BCDC_BUF_MAX {
            return Err(Error::Protocol("command too long"));
        }
        let reqid = self.reqid;
        self.reqid = self.reqid.wrapping_add(1).max(1);
        let msg = bcdc::encode(cmd, set, reqid, 0, data);
        self.wait_credit()?;
        self.bus.tx_frame(Channel::Control, None, &msg)?;
        let start = self.bus.sdio.now_ms();
        loop {
            self.service()?;
            if let Some(i) = self.ctl.iter().position(|m| bcdc::decode(m).is_some_and(|(d, _)| d.reqid() == reqid)) {
                let m = self.ctl.remove(i).unwrap_or_default();
                let (d, body) = bcdc::decode(&m).ok_or(Error::Protocol("control response"))?;
                if d.error() {
                    return Err(Error::Status(d.status as i32));
                }
                return Ok((d, body.to_vec()));
            }
            if self.bus.sdio.now_ms() - start > CONTROL_TIMEOUT_MS {
                return Err(Error::Timeout("control response"));
            }
            self.bus.sdio.delay_us(2000);
        }
    }

    /// Until the firmware has credit for a frame (a few passes of serving
    /// the card).
    fn wait_credit(&mut self) -> Result<(), Error> {
        for _ in 0..200 {
            if self.bus.tx_ok() {
                return Ok(());
            }
            self.service()?;
            self.bus.sdio.delay_us(1000);
        }
        Err(Error::Timeout("transmit credit"))
    }

    pub fn cmd_get(&mut self, cmd: u32, len: usize) -> Result<Vec<u8>, Error> {
        let (_, body) = self.control(cmd, false, &vec![0u8; len])?;
        Ok(body)
    }

    pub fn cmd_set(&mut self, cmd: u32, data: &[u8]) -> Result<(), Error> {
        self.control(cmd, true, data).map(|_| ())
    }

    pub fn cmd_get_int(&mut self, cmd: u32) -> Result<u32, Error> {
        let v = self.cmd_get(cmd, 4)?;
        Ok(u32::from_le_bytes([v[0], v[1], v[2], v[3]]))
    }

    pub fn cmd_set_int(&mut self, cmd: u32, value: u32) -> Result<(), Error> {
        self.cmd_set(cmd, &value.to_le_bytes())
    }

    /// Variable `name`: `len` bytes of its value.
    pub fn var_get(&mut self, name: &str, len: usize) -> Result<Vec<u8>, Error> {
        let (_, body) = self.control(C_GET_VAR, false, &bcdc::var(name, &vec![0u8; len]))?;
        if body.len() < len {
            return Err(Error::Protocol("short variable"));
        }
        Ok(body[..len].to_vec())
    }

    pub fn var_set(&mut self, name: &str, data: &[u8]) -> Result<(), Error> {
        self.control(C_SET_VAR, true, &bcdc::var(name, data)).map(|_| ())
    }

    pub fn var_get_int(&mut self, name: &str) -> Result<u32, Error> {
        let v = self.var_get(name, 4)?;
        Ok(u32::from_le_bytes([v[0], v[1], v[2], v[3]]))
    }

    pub fn var_set_int(&mut self, name: &str, value: u32) -> Result<(), Error> {
        self.var_set(name, &value.to_le_bytes())
    }

    // ---- Serving the card -----------------------------------------------------

    /// Serve the card: what it sent goes to its queue (a control response,
    /// an event, an Ethernet frame for the host), the events handled.
    pub fn service(&mut self) -> Result<(), Error> {
        let (ctl, events, rx) = (&mut self.ctl, &mut self.events, &mut self.rx);
        let r = self.bus.service(&mut |channel, payload| match channel {
            Channel::Control => ctl.push_back(payload.to_vec()),
            Channel::Event => events.push_back(payload.to_vec()),
            Channel::Data => rx.push_back(payload.to_vec()),
        });
        self.handle_frames();
        r
    }

    /// The events received, and the data frames that are events in
    /// disguise (the firmware tells them apart; the data channel's keep
    /// their BCDC header until here).
    fn handle_frames(&mut self) {
        while let Some(f) = self.events.pop_front() {
            if let Some(eth) = bcdc::payload(&f) {
                self.handle_event(eth);
            }
        }
        let mut kept = VecDeque::new();
        while let Some(f) = self.rx.pop_front() {
            let Some(eth) = bcdc::payload(&f) else {
                continue;
            };
            if event::parse(eth).is_some() {
                self.handle_event(eth);
            } else if self.link() == Link::Up && kept.len() < RX_QUEUE {
                kept.push_back(eth.to_vec());
            }
        }
        self.rx = kept;
    }

    fn handle_event(&mut self, frame: &[u8]) {
        let Some(e) = event::parse(frame) else {
            return;
        };
        match e.kind {
            E_ESCAN_RESULT => {
                if e.status == E_STATUS_PARTIAL {
                    if let Some(found) = self.scan.as_mut() {
                        for s in event::escan_results(e.data, self.io_type) {
                            match found.iter_mut().find(|f| f.bssid == s.bssid) {
                                Some(f) if f.rssi < s.rssi => *f = s,
                                Some(_) => {}
                                None => found.push(s),
                            }
                        }
                    }
                } else if let Some(found) = self.scan.take() {
                    self.scanned = Some(found);
                }
            }
            E_SET_SSID => {
                if e.status == E_STATUS_SUCCESS {
                    self.bssid = Some(e.addr);
                } else if self.link == Link::Joining {
                    self.link = Link::Failed(e.status);
                }
            }
            E_LINK => {
                self.link_up = e.flags & EVENT_FLAG_LINK_UP != 0;
                if !self.link_up && self.link == Link::Up {
                    self.link = Link::Down;
                }
            }
            E_PSK_SUP => {
                if e.status == WLC_SUP_KEYED {
                    self.keyed = true;
                }
            }
            E_DEAUTH | E_DEAUTH_IND | E_DISASSOC | E_DISASSOC_IND => {
                self.link_up = false;
                if self.link == Link::Up {
                    self.link = Link::Down;
                }
            }
            _ => {}
        }
        if self.link == Link::Joining && self.link_up && (!self.psk || self.keyed) {
            self.link = Link::Up;
        }
    }

    // ---- Scanning --------------------------------------------------------------

    /// Start a scan of every channel (the firmware's escan); its results
    /// come in events, [`Dev::scan_results`] has them once it ends.
    pub fn scan(&mut self) -> Result<(), Error> {
        let mut p = Vec::with_capacity(96);
        let scan_params = |p: &mut Vec<u8>, v2: bool| {
            if v2 {
                // version, length (filled in below).
                p.extend_from_slice(&2u16.to_le_bytes());
                p.extend_from_slice(&0u16.to_le_bytes());
            }
            // The SSID (none: a wildcard), any BSSID, any BSS type.
            p.extend_from_slice(&[0u8; SSID_LEN]);
            p.extend_from_slice(&[0xff; 6]);
            p.push(BSSTYPE_ANY);
            if v2 {
                p.push(0);
                p.extend_from_slice(&u32::from(SCANTYPE_ACTIVE).to_le_bytes());
            } else {
                p.push(SCANTYPE_ACTIVE);
            }
            // Probes, times: the firmware's defaults; every channel.
            for _ in 0..4 {
                p.extend_from_slice(&u32::MAX.to_le_bytes());
            }
            p.extend_from_slice(&0u32.to_le_bytes());
        };
        let version = if self.scan_ver == 0 { ESCAN_REQ_VERSION } else { ESCAN_REQ_VERSION_V2 };
        p.extend_from_slice(&version.to_le_bytes());
        p.extend_from_slice(&ESCAN_ACTION_START.to_le_bytes());
        p.extend_from_slice(&0x1234u16.to_le_bytes());
        let at = p.len();
        scan_params(&mut p, self.scan_ver != 0);
        if self.scan_ver != 0 {
            let len = (p.len() - at) as u16;
            p[at + 2..at + 4].copy_from_slice(&len.to_le_bytes());
        }
        self.scanned = None;
        self.scan = Some(Vec::new());
        if let Err(e) = self.var_set("escan", &p) {
            self.scan = None;
            return Err(e);
        }
        Ok(())
    }

    /// A scan is under way.
    pub fn scanning(&self) -> bool {
        self.scan.is_some()
    }

    /// The last scan's networks, once it ended (then taken).
    pub fn scan_results(&mut self) -> Option<Vec<Scanned>> {
        self.scanned.take()
    }

    // ---- The link --------------------------------------------------------------

    /// Join network `ssid` at `bssid` on `channel`: an open one, or with
    /// the firmware's supplicant and `pmk` a WPA2-PSK one. The link's
    /// state follows in [`Dev::link`].
    pub fn join(&mut self, ssid: &[u8], bssid: [u8; 6], channel: u8, pmk: Option<&[u8; WSEC_MAX_PSK_LEN]>) -> Result<(), Error> {
        if ssid.is_empty() || ssid.len() > MAX_SSID_LEN {
            return Err(Error::Protocol("SSID length"));
        }
        self.link = Link::Down;
        self.link_up = false;
        self.keyed = false;
        self.psk = pmk.is_some();
        match pmk {
            Some(pmk) => {
                self.var_set_int("wpa_auth", WPA_AUTH_WPA2_PSK)?;
                self.var_set_int("wsec", WSEC_AES)?;
                // The PMK as 64 hex digits, as a passphrase would be.
                let mut k = Vec::with_capacity(4 + 2 * WSEC_MAX_PSK_LEN + 1);
                k.extend_from_slice(&(2 * WSEC_MAX_PSK_LEN as u16).to_le_bytes());
                k.extend_from_slice(&WSEC_PASSPHRASE.to_le_bytes());
                for b in pmk {
                    k.extend_from_slice(&[b"0123456789abcdef"[usize::from(b >> 4)], b"0123456789abcdef"[usize::from(b & 15)]]);
                }
                k.push(0);
                self.cmd_set(C_SET_WSEC_PMK, &k)?;
            }
            None => {
                self.var_set_int("wpa_auth", WPA_AUTH_DISABLED)?;
                self.var_set_int("wsec", WSEC_NONE)?;
            }
        }
        self.var_set_int("auth", AUTH_OPEN)?;
        self.var_set_int("mfp", MFP_NONE)?;
        let chanspec = event::chan2spec(channel, self.io_type);
        // ext_join_params: the SSID, the join scan's parameters (the
        // firmware's defaults), the BSSID and its channel.
        let mut p = Vec::with_capacity(72);
        p.extend_from_slice(&(ssid.len() as u32).to_le_bytes());
        p.extend_from_slice(ssid);
        p.resize(SSID_LEN, 0);
        p.extend_from_slice(&[0xff, 0, 0, 0]);
        for _ in 0..4 {
            p.extend_from_slice(&u32::MAX.to_le_bytes());
        }
        p.extend_from_slice(&bssid);
        p.extend_from_slice(&[0, 0]);
        p.extend_from_slice(&1u32.to_le_bytes());
        p.extend_from_slice(&chanspec.to_le_bytes());
        p.extend_from_slice(&[0, 0]);
        self.link = Link::Joining;
        self.bssid = Some(bssid);
        match self.var_set("join", &p) {
            Ok(()) => Ok(()),
            Err(Error::Status(_)) => {
                // An older firmware: the SSID alone, the BSSID beside it.
                let mut j = Vec::with_capacity(52);
                j.extend_from_slice(&p[..SSID_LEN]);
                j.extend_from_slice(&p[SSID_LEN + 20..]);
                self.cmd_set(C_SET_SSID, &j)
            }
            Err(e) => {
                self.link = Link::Down;
                Err(e)
            }
        }
    }

    pub fn link(&self) -> Link {
        self.link
    }

    /// The network's BSSID, while joining or joined.
    pub fn bssid(&self) -> Option<[u8; 6]> {
        self.bssid
    }

    /// Leave the network.
    pub fn leave(&mut self) -> Result<(), Error> {
        self.link = Link::Down;
        self.link_up = false;
        self.bssid = None;
        self.rx.clear();
        self.cmd_set(C_DISASSOC, &[])
    }

    // ---- Frames --------------------------------------------------------------

    /// An Ethernet frame to the network.
    pub fn send(&mut self, frame: &[u8]) -> Result<(), Error> {
        self.wait_credit()?;
        self.bus.tx_frame(Channel::Data, Some(bcdc::data_header(0)), frame)
    }

    /// An Ethernet frame from the network, if one came ([`Dev::service`]
    /// brings them).
    pub fn recv(&mut self) -> Option<Vec<u8>> {
        self.rx.pop_front()
    }

    /// Frames wait to be taken.
    pub fn pending(&self) -> bool {
        !self.rx.is_empty()
    }
}
