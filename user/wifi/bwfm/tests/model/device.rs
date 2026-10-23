//! The chip: a CYW43455 (an ARM CR4 with its own RAM) or a CYW43438 (an
//! ARM CM3 with a SOCRAM core), as `bwfm::sdio::Sdio`.

use std::collections::{BTreeMap, VecDeque};

use bwfm::bcdc;
use bwfm::event;
use bwfm::reg::*;
use bwfm::sdio::{Sdio, SdioError};

pub const MAC: [u8; 6] = [0xb8, 0x27, 0xeb, 0x12, 0x34, 0x56];

/// Which chip the model is.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    Cyw43455,
    Cyw43438,
}

/// A network in the model's air.
#[derive(Clone, Debug)]
pub struct Network {
    pub bssid: [u8; 6],
    pub ssid: &'static [u8],
    pub channel: u8,
    pub rssi: i16,
    /// WPA2-PSK with this key, or open.
    pub psk: Option<[u8; 32]>,
}

/// The chipcommon core, the SDIO device core, the CPU and the 802.11 core:
/// their registers' bases and their agents'.
const CC: u32 = 0x1800_0000;
const CC_WRAP: u32 = 0x1810_0000;
const D11: u32 = 0x1800_1000;
const D11_WRAP: u32 = 0x1810_1000;
const SDIOD: u32 = 0x1800_2000;
const SDIOD_WRAP: u32 = 0x1810_2000;
const CPU: u32 = 0x1800_3000;
const CPU_WRAP: u32 = 0x1810_3000;
const SOCRAM: u32 = 0x1800_4000;
const SOCRAM_WRAP: u32 = 0x1810_4000;
/// Where the enumeration ROM is.
const EROM: u32 = 0x1800_0800;
/// The RAM: 512 KiB.
pub const RAMSIZE: u32 = 0x8_0000;

pub struct Device {
    pub kind: Kind,
    /// Function 0's registers (the CCCR, the FBRs).
    f0: [u8; 0x200],
    /// Function 1's own (`0x10000` and up).
    f1: [u8; 0x40],
    pub block: [u16; 8],
    /// The backplane: 32-bit registers, and the RAM.
    regs: BTreeMap<u32, u32>,
    pub rambase: u32,
    pub ram: Vec<u8>,
    bar0: u32,
    /// The firmware is running (the CPU was started on the RAM's image).
    pub running: bool,
    /// Frames for the host, each a whole SDPCM frame, and how far into the
    /// first the host has read.
    to_host: VecDeque<Vec<u8>>,
    cursor: usize,
    /// Frames a command raises, held back until its reply is out and the
    /// host has drained the queue: the firmware answers first, then acts.
    deferred: Option<Vec<Vec<u8>>>,
    later: Vec<Vec<u8>>,
    /// The last sequence number the host sent.
    host_seq: u8,
    pub irq: bool,
    /// What the host asked and sent, for the tests.
    pub vars: BTreeMap<String, Vec<u8>>,
    pub cmds: Vec<(u32, Vec<u8>)>,
    pub clm: Vec<u8>,
    pub pmk_hex: Option<Vec<u8>>,
    pub sent: Vec<Vec<u8>>,
    pub air: Vec<Network>,
    pub joined: Option<[u8; 6]>,
    pub booted_with: Option<Vec<u8>>,
}

const IO_TYPE: u32 = IO_TYPE_D11AC;
/// BCME_UNSUPPORTED.
const UNSUPPORTED: u32 = (-23i32) as u32;

fn rsn_psk_ie() -> Vec<u8> {
    vec![0x30, 0x14, 1, 0, 0x00, 0x0f, 0xac, 4, 1, 0, 0x00, 0x0f, 0xac, 4, 1, 0, 0x00, 0x0f, 0xac, 2, 0, 0]
}

impl Device {
    pub fn new(kind: Kind) -> Device {
        let mut d = Device {
            kind,
            f0: [0; 0x200],
            f1: [0; 0x40],
            block: [0; 8],
            regs: BTreeMap::new(),
            rambase: if kind == Kind::Cyw43455 { 0x19_8000 } else { 0 },
            ram: vec![0; RAMSIZE as usize],
            bar0: 0,
            running: false,
            to_host: VecDeque::new(),
            cursor: 0,
            deferred: None,
            later: Vec::new(),
            host_seq: 0,
            irq: false,
            vars: BTreeMap::new(),
            cmds: Vec::new(),
            clm: Vec::new(),
            pmk_hex: None,
            sent: Vec::new(),
            air: vec![
                Network { bssid: [0x02, 0x00, 0x5e, 0x00, 0x01, 0x01], ssid: b"myos-open", channel: 1, rssi: -40, psk: None },
                Network { bssid: [0x02, 0x00, 0x5e, 0x00, 0x02, 0x02], ssid: b"myos-psk", channel: 6, rssi: -55, psk: Some([0x5a; 32]) },
            ],
            joined: None,
            booted_with: None,
        };
        // The CCCR: SDIO revision, speed, Broadcom's card control.
        d.f0[0] = 0x43;
        d.f0[0x13] = 0x01;
        // Chipcommon: the chip id (AI interconnect), a PMU, the EROM.
        let (id, rev) = match kind {
            Kind::Cyw43455 => (0x4345u32, 6u32),
            Kind::Cyw43438 => (43430, 1),
        };
        d.regs.insert(CC + CHIP_REG_CHIPID, id | rev << 16 | CHIPID_TYPE_SOCI_AI << 28);
        d.regs.insert(CC + CHIP_REG_CAPABILITIES, CHIP_REG_CAPABILITIES_PMU);
        d.regs.insert(CC + CHIP_REG_EROMPTR, EROM);
        d.regs.insert(CC + CHIP_REG_PMUCAPABILITIES, 24);
        // The cores: component descriptors, a slave port and a wrapper each.
        let mut cores = vec![(CORE_CHIPCOMMON, 30u8, CC, CC_WRAP), (CORE_80211, 50, D11, D11_WRAP), (CORE_SDIO_DEV, 20, SDIOD, SDIOD_WRAP)];
        match kind {
            Kind::Cyw43455 => cores.push((CORE_ARM_CR4, 7, CPU, CPU_WRAP)),
            Kind::Cyw43438 => {
                cores.push((CORE_ARM_CM3, 4, CPU, CPU_WRAP));
                cores.push((CORE_INTERNAL_MEM, 10, SOCRAM, SOCRAM_WRAP));
            }
        }
        let mut at = EROM;
        for (id, rev, base, wrap) in cores {
            for w in [
                u32::from(id) << DMP_COMP_PARTNUM_S | DMP_DESC_COMPONENT,
                u32::from(rev) << DMP_COMP_REVISION_S | 1 << DMP_COMP_NUM_SWRAP_S | DMP_DESC_COMPONENT,
                base | DMP_SLAVE_TYPE_SLAVE << DMP_SLAVE_TYPE_S | DMP_SLAVE_SIZE_4K << DMP_SLAVE_SIZE_TYPE_S | DMP_DESC_ADDRESS,
                wrap | DMP_SLAVE_TYPE_SWRAP << DMP_SLAVE_TYPE_S | DMP_SLAVE_SIZE_4K << DMP_SLAVE_SIZE_TYPE_S | DMP_DESC_ADDRESS,
            ] {
                d.regs.insert(at, w);
                at += 4;
            }
            // Every agent starts in reset.
            d.regs.insert(wrap + AGENT_RESET_CTL, AGENT_RESET_CTL_RESET);
        }
        d.regs.insert(at, DMP_DESC_EOT);
        match kind {
            // One TCM bank of 64 blocks of 8 KiB.
            Kind::Cyw43455 => {
                d.regs.insert(CPU + ARMCR4_CAP, 1);
                d.regs.insert(CPU + ARMCR4_BANKINFO, RAMSIZE / 8192 - 1);
            }
            // Eight SOCRAM banks of 64 KiB.
            Kind::Cyw43438 => {
                d.regs.insert(SOCRAM + SOCRAM_COREINFO, 8 << SOCRAM_COREINFO_SRNB_SHIFT);
                d.regs.insert(SOCRAM + SOCRAM_BANKINFO, 64 * 1024 / SOCRAM_BANKINFO_SZBASE - 1);
            }
        }
        d
    }

    // ---- The backplane ------------------------------------------------------

    fn in_ram(&self, addr: u32) -> Option<usize> {
        let off = addr.checked_sub(self.rambase)?;
        (off < RAMSIZE).then_some(off as usize)
    }

    fn bp_read32(&mut self, addr: u32) -> u32 {
        if let Some(off) = self.in_ram(addr) {
            let r = &self.ram[off..off + 4];
            return u32::from_le_bytes([r[0], r[1], r[2], r[3]]);
        }
        self.regs.get(&addr).copied().unwrap_or(0)
    }

    fn bp_write32(&mut self, addr: u32, v: u32) {
        if let Some(off) = self.in_ram(addr) {
            self.ram[off..off + 4].copy_from_slice(&v.to_le_bytes());
            return;
        }
        match addr {
            // Write 1 to clear.
            a if a == SDIOD + SDPCMD_INTSTATUS => {
                let cur = self.regs.get(&addr).copied().unwrap_or(0);
                self.regs.insert(addr, cur & !v);
            }
            a if a == SDIOD + SDPCMD_TOSBMAILBOX => {}
            // The CPU's agent: clocked and out of reset without CPUHALT,
            // the firmware starts.
            a if a == CPU_WRAP + AGENT_IOCTL => {
                self.regs.insert(addr, v);
                let in_reset = self.regs.get(&(CPU_WRAP + AGENT_RESET_CTL)).copied().unwrap_or(0) & AGENT_RESET_CTL_RESET != 0;
                if !in_reset && v & AGENT_IOCTL_CLK != 0 && v & AGENT_IOCTL_ARMCR4_CPUHALT == 0 {
                    self.boot();
                } else if v & AGENT_IOCTL_ARMCR4_CPUHALT != 0 || in_reset {
                    self.running = false;
                }
            }
            a if a == CPU_WRAP + AGENT_RESET_CTL => {
                self.regs.insert(addr, v);
                if v & AGENT_RESET_CTL_RESET != 0 {
                    self.running = false;
                }
            }
            _ => {
                self.regs.insert(addr, v);
            }
        }
    }

    /// The firmware starts: it remembers what it was started on, says it
    /// is ready through the mailbox, and leaves its shared structure at
    /// the end of RAM.
    fn boot(&mut self) {
        self.running = true;
        self.booted_with = Some(self.ram.clone());
        let shared = RAMSIZE - 64;
        let console = RAMSIZE - 128;
        self.ram[shared as usize..shared as usize + 4].copy_from_slice(&3u32.to_le_bytes());
        self.ram[shared as usize + SDPCM_SHARED_CONSOLE_ADDR..shared as usize + SDPCM_SHARED_CONSOLE_ADDR + 4].copy_from_slice(&(self.rambase + console).to_le_bytes());
        let end = (RAMSIZE - 4) as usize;
        self.ram[end..end + 4].copy_from_slice(&(self.rambase + shared).to_le_bytes());
        self.regs.insert(SDIOD + SDPCMD_TOHOSTMAILBOXDATA, SDPCMD_TOHOSTMAILBOXDATA_FWREADY | SDPCMD_TOHOSTMAILBOXDATA_DEVREADY);
        let cur = self.regs.get(&(SDIOD + SDPCMD_INTSTATUS)).copied().unwrap_or(0);
        self.regs.insert(SDIOD + SDPCMD_INTSTATUS, cur | SDPCMD_INTSTATUS_HMB_HOST_INT);
        self.irq = true;
    }

    // ---- Frames to the host ----------------------------------------------------

    /// An SDPCM frame to the host on `channel` with `payload`.
    fn frame_to_host(&mut self, channel: u8, payload: &[u8]) {
        let len = SDPCM_HDR_LEN + payload.len();
        let mut f = Vec::with_capacity(len.next_multiple_of(4));
        f.extend_from_slice(&(len as u16).to_le_bytes());
        f.extend_from_slice(&(!(len as u16)).to_le_bytes());
        // Credits: 8 frames past the last the host sent.
        f.extend_from_slice(&[0, channel, 0, SDPCM_HDR_LEN as u8, 0, self.host_seq.wrapping_add(8), 0, 0]);
        f.extend_from_slice(payload);
        f.resize(len.next_multiple_of(4), 0);
        if let Some(d) = self.deferred.as_mut() {
            d.push(f);
            return;
        }
        self.to_host.push_back(f);
        let cur = self.regs.get(&(SDIOD + SDPCMD_INTSTATUS)).copied().unwrap_or(0);
        self.regs.insert(SDIOD + SDPCMD_INTSTATUS, cur | SDPCMD_INTSTATUS_HMB_FRAME_IND);
        self.irq = true;
    }

    fn event_to_host(&mut self, kind: u32, status: u32, reason: u32, flags: u16, addr: [u8; 6], data: &[u8]) {
        let mut p = Vec::from(bcdc::data_header(0));
        p.extend_from_slice(&event::build(MAC, kind, status, reason, flags, addr, data));
        self.frame_to_host(SDPCM_CHANNEL_EVENT, &p);
    }

    /// An Ethernet frame from the network to the host.
    pub fn inject(&mut self, eth: &[u8]) {
        let mut p = Vec::from(bcdc::data_header(0));
        p.extend_from_slice(eth);
        self.frame_to_host(SDPCM_CHANNEL_DATA, &p);
    }

    /// The network lets the station go.
    pub fn disconnect(&mut self) {
        let bssid = self.joined.take().unwrap_or([0; 6]);
        self.event_to_host(E_LINK, E_STATUS_SUCCESS, 0, 0, bssid, &[]);
        self.event_to_host(E_DEAUTH_IND, E_STATUS_SUCCESS, 2, 0, bssid, &[]);
    }

    // ---- Frames from the host -----------------------------------------------------

    fn frame_from_host(&mut self, f: &[u8]) {
        assert!(f.len() >= SDPCM_HDR_LEN, "a frame has its headers");
        let len = usize::from(u16::from_le_bytes([f[0], f[1]]));
        let cksum = u16::from_le_bytes([f[2], f[3]]);
        assert_eq!(len as u16 ^ cksum, 0xffff, "the hardware header's checksum");
        assert!(len <= f.len(), "the frame is as long as its header says");
        assert_eq!(f.len() % if len > 512 { 512 } else { 4 }, 0, "padded to a word, or past 512 bytes to a block");
        self.host_seq = f[4];
        let channel = f[5] & SDPCM_CHANNEL_MASK;
        let dataoff = usize::from(f[7]);
        assert!(dataoff >= SDPCM_HDR_LEN && dataoff <= len);
        let payload = f[dataoff..len].to_vec();
        match channel {
            SDPCM_CHANNEL_CONTROL => self.control(&payload),
            SDPCM_CHANNEL_DATA => {
                let eth = bcdc::payload(&payload).expect("a BCDC header").to_vec();
                assert_eq!(payload[0] >> 4, BCDC_FLAG_PROTO_VER);
                self.sent.push(eth);
            }
            _ => panic!("frame on channel {channel}"),
        }
    }

    /// A control message: done, and answered.
    fn control(&mut self, msg: &[u8]) {
        let (d, data) = bcdc::decode(msg).expect("a BCDC control message");
        assert!(self.running, "a command while the firmware is not running");
        assert_eq!(d.len as usize, data.len());
        let set = d.flags & BCDC_DCMD_SET != 0;
        let mut out = data.to_vec();
        self.deferred = Some(Vec::new());
        let status = self.dcmd(d.cmd, set, &mut out);
        let raised = self.deferred.take().unwrap();
        let mut resp = Vec::with_capacity(BCDC_DCMD_HDR_LEN + out.len());
        resp.extend_from_slice(&d.cmd.to_le_bytes());
        resp.extend_from_slice(&(out.len() as u32).to_le_bytes());
        resp.extend_from_slice(&(d.flags | if status != 0 { BCDC_DCMD_ERROR } else { 0 }).to_le_bytes());
        resp.extend_from_slice(&status.to_le_bytes());
        resp.extend_from_slice(&out);
        self.frame_to_host(SDPCM_CHANNEL_CONTROL, &resp);
        self.later.extend(raised);
    }

    /// The firmware's commands: the status (0: fine).
    fn dcmd(&mut self, cmd: u32, set: bool, data: &mut Vec<u8>) -> u32 {
        self.cmds.push((cmd, data.clone()));
        match cmd {
            C_GET_VERSION => {
                data[..4].copy_from_slice(&IO_TYPE.to_le_bytes());
                0
            }
            C_GET_BANDLIST => {
                data[..8].copy_from_slice(&[1, 0, 0, 0, 2, 0, 0, 0]);
                0
            }
            C_GET_VAR => {
                let nul = data.iter().position(|&c| c == 0).expect("a variable name");
                let name = String::from_utf8_lossy(&data[..nul]).into_owned();
                let value = &mut data[nul + 1..];
                match name.as_str() {
                    "cur_etheraddr" => value[..6].copy_from_slice(&MAC),
                    "scan_ver" | "nmode" | "vhtmode" => value[..4].copy_from_slice(&0u32.to_le_bytes()),
                    _ => return UNSUPPORTED,
                }
                // The firmware answers the value alone.
                *data = value.to_vec();
                0
            }
            C_SET_VAR => {
                assert!(set);
                let nul = data.iter().position(|&c| c == 0).expect("a variable name");
                let name = String::from_utf8_lossy(&data[..nul]).into_owned();
                let value = data[nul + 1..].to_vec();
                self.vars.insert(name.clone(), value.clone());
                match name.as_str() {
                    "clmload" => {
                        let flag = u16::from_le_bytes([value[0], value[1]]);
                        if flag & DLOAD_FLAG_BEGIN != 0 {
                            self.clm.clear();
                        }
                        assert_eq!(u16::from_le_bytes([value[2], value[3]]), DLOAD_TYPE_CLM);
                        let len = u32::from_le_bytes([value[4], value[5], value[6], value[7]]) as usize;
                        self.clm.extend_from_slice(&value[DLOAD_HDR_LEN..DLOAD_HDR_LEN + len]);
                        0
                    }
                    "escan" => self.escan(&value),
                    "join" => self.join(&value),
                    "mpc" | "join_pref" | "event_msgs" | "txbf" | "arp_ol" | "arpoe" | "ndoe" | "toe" | "sup_wpa" | "wpa_auth" | "wsec" | "auth" | "mfp" => 0,
                    _ => UNSUPPORTED,
                }
            }
            C_SET_WSEC_PMK => {
                let key_len = usize::from(u16::from_le_bytes([data[0], data[1]]));
                assert_eq!(u16::from_le_bytes([data[2], data[3]]), WSEC_PASSPHRASE);
                self.pmk_hex = Some(data[4..4 + key_len].to_vec());
                0
            }
            C_SET_SSID => {
                let mut v = data.clone();
                // join_params: the SSID, then the assoc params; as a join.
                v.splice(SSID_LEN..SSID_LEN, [0u8; 20]);
                self.join(&v)
            }
            C_DISASSOC => {
                if let Some(bssid) = self.joined.take() {
                    self.event_to_host(E_LINK, E_STATUS_SUCCESS, 0, 0, bssid, &[]);
                }
                0
            }
            C_UP | C_SET_INFRA | C_SET_AP | C_SET_PM | C_SET_SCAN_CHANNEL_TIME | C_SET_SCAN_UNASSOC_TIME | C_SET_SCAN_PASSIVE_TIME => 0,
            _ => UNSUPPORTED,
        }
    }

    /// A scan: every network, one result event each, then the end.
    fn escan(&mut self, params: &[u8]) -> u32 {
        assert_eq!(u32::from_le_bytes([params[0], params[1], params[2], params[3]]), ESCAN_REQ_VERSION);
        let action = u16::from_le_bytes([params[4], params[5]]);
        if action == ESCAN_ACTION_ABORT {
            self.event_to_host(E_ESCAN_RESULT, E_STATUS_ABORT, 0, 0, [0; 6], &[]);
            return 0;
        }
        assert_eq!(action, ESCAN_ACTION_START);
        let air = self.air.clone();
        for n in &air {
            let mut ies = vec![0u8, n.ssid.len() as u8];
            ies.extend_from_slice(n.ssid);
            let capability = if n.psk.is_some() { 0x0411 } else { 0x0401 };
            if n.psk.is_some() {
                ies.extend_from_slice(&rsn_psk_ie());
            }
            let rec = event::build_bss_info(n.bssid, n.ssid, event::chan2spec(n.channel, IO_TYPE), n.rssi, capability, &ies);
            let data = event::build_escan_results(&[rec]);
            self.event_to_host(E_ESCAN_RESULT, E_STATUS_PARTIAL, 0, 0, [0; 6], &data);
        }
        self.event_to_host(E_ESCAN_RESULT, E_STATUS_SUCCESS, 0, 0, [0; 6], &[]);
        0
    }

    /// A join (ext_join_params): the network looked for; the events of
    /// its outcome.
    fn join(&mut self, params: &[u8]) -> u32 {
        let ssid_len = u32::from_le_bytes([params[0], params[1], params[2], params[3]]) as usize;
        let ssid = params[4..4 + ssid_len].to_vec();
        let assoc = &params[SSID_LEN + 20..];
        let bssid: [u8; 6] = assoc[..6].try_into().unwrap();
        let chanspec_num = u32::from_le_bytes([assoc[8], assoc[9], assoc[10], assoc[11]]);
        let net = self.air.iter().find(|n| n.ssid == &ssid[..] && (bssid == [0; 6] || n.bssid == bssid)).cloned();
        let Some(net) = net else {
            self.event_to_host(E_SET_SSID, E_STATUS_NO_NETWORKS, 0, 0, bssid, &[]);
            return 0;
        };
        if chanspec_num != 0 {
            let spec = u32::from(u16::from_le_bytes([assoc[12], assoc[13]]));
            assert_eq!(event::spec2chan(spec, IO_TYPE), net.channel, "the join names the network's channel");
        }
        let wpa_auth = self.vars.get("wpa_auth").map(|v| u32::from_le_bytes([v[0], v[1], v[2], v[3]])).unwrap_or(0);
        match (net.psk, wpa_auth) {
            (Some(psk), WPA_AUTH_WPA2_PSK) => {
                let hex: Vec<u8> = psk.iter().flat_map(|b| [b"0123456789abcdef"[usize::from(b >> 4)], b"0123456789abcdef"[usize::from(b & 15)]]).collect();
                let sup = self.vars.get("sup_wpa").is_some_and(|v| v[0] == 1);
                if !sup || self.pmk_hex.as_deref() != Some(&hex[..]) {
                    self.event_to_host(E_SET_SSID, E_STATUS_SUCCESS, 0, 0, net.bssid, &[]);
                    self.event_to_host(E_LINK, E_STATUS_SUCCESS, 0, EVENT_FLAG_LINK_UP, net.bssid, &[]);
                    // The handshake fails: the access point throws us out.
                    self.event_to_host(E_DEAUTH_IND, E_STATUS_SUCCESS, 15, 0, net.bssid, &[]);
                    self.event_to_host(E_LINK, E_STATUS_SUCCESS, 15, 0, net.bssid, &[]);
                    return 0;
                }
                self.joined = Some(net.bssid);
                self.event_to_host(E_SET_SSID, E_STATUS_SUCCESS, 0, 0, net.bssid, &[]);
                self.event_to_host(E_LINK, E_STATUS_SUCCESS, 0, EVENT_FLAG_LINK_UP, net.bssid, &[]);
                self.event_to_host(E_PSK_SUP, WLC_SUP_KEYED, 0, 0, net.bssid, &[]);
            }
            (Some(_), _) => {
                self.event_to_host(E_SET_SSID, E_STATUS_FAIL, 0, 0, net.bssid, &[]);
            }
            (None, _) => {
                self.joined = Some(net.bssid);
                self.event_to_host(E_SET_SSID, E_STATUS_SUCCESS, 0, 0, net.bssid, &[]);
                self.event_to_host(E_LINK, E_STATUS_SUCCESS, 0, EVENT_FLAG_LINK_UP, net.bssid, &[]);
            }
        }
        0
    }

    fn f1_reg(addr: u32) -> usize {
        (addr - 0x10000) as usize
    }
}

impl Sdio for Device {
    fn read_1(&mut self, func: u8, addr: u32) -> Result<u8, SdioError> {
        match func {
            0 => Ok(match addr {
                // I/O ready follows I/O enable.
                0x03 => self.f0[0x02],
                a if (a as usize) < self.f0.len() => self.f0[a as usize],
                _ => 0,
            }),
            1 if addr >= 0x10000 && addr < 0x10040 => {
                let v = self.f1[Self::f1_reg(addr)];
                if addr == FUNC1_CHIPCLKCSR {
                    // The clocks asked for are there.
                    let mut avail = 0;
                    if v & (CHIPCLKCSR_ALP_AVAIL_REQ | CHIPCLKCSR_FORCE_ALP | CHIPCLKCSR_HT_AVAIL_REQ | CHIPCLKCSR_FORCE_HT) != 0 {
                        avail |= CHIPCLKCSR_ALP_AVAIL;
                    }
                    if v & (CHIPCLKCSR_HT_AVAIL_REQ | CHIPCLKCSR_FORCE_HT) != 0 {
                        avail |= CHIPCLKCSR_HT_AVAIL;
                    }
                    return Ok(v & 0x3f | avail);
                }
                Ok(v)
            }
            _ => Err(SdioError),
        }
    }

    fn write_1(&mut self, func: u8, addr: u32, value: u8) -> Result<(), SdioError> {
        match func {
            0 if (addr as usize) < self.f0.len() => {
                self.f0[addr as usize] = value;
                Ok(())
            }
            1 if addr >= 0x10000 && addr < 0x10040 => {
                self.f1[Self::f1_reg(addr)] = value;
                let low = u32::from(self.f1[Self::f1_reg(FUNC1_SBADDRLOW)] & 0x80) << 8;
                let mid = u32::from(self.f1[Self::f1_reg(FUNC1_SBADDRMID)]) << 16;
                let high = u32::from(self.f1[Self::f1_reg(FUNC1_SBADDRHIGH)]) << 24;
                self.bar0 = low | mid | high;
                Ok(())
            }
            _ => Err(SdioError),
        }
    }

    fn read(&mut self, func: u8, addr: u32, buf: &mut [u8], incr: bool) -> Result<(), SdioError> {
        assert_eq!(buf.len() % 4, 0, "whole words");
        match func {
            1 => {
                assert!(incr && addr & SB_ACCESS_2_4B_FLAG != 0, "a backplane access");
                let base = self.bar0 | (addr & SB_OFT_ADDR_MASK);
                for (i, w) in buf.chunks_mut(4).enumerate() {
                    w.copy_from_slice(&self.bp_read32(base + 4 * i as u32).to_le_bytes());
                }
                Ok(())
            }
            2 => {
                assert!(!incr, "frames are read at a fixed address");
                assert!(self.f0[0x02] & 4 != 0, "function 2 is enabled");
                let mut done = 0;
                while done < buf.len() {
                    let Some(front) = self.to_host.front() else {
                        buf[done..].fill(0);
                        // The queue is drained: what the last command
                        // raised comes next.
                        if !self.later.is_empty() {
                            self.to_host.extend(std::mem::take(&mut self.later));
                            let cur = self.regs.get(&(SDIOD + SDPCMD_INTSTATUS)).copied().unwrap_or(0);
                            self.regs.insert(SDIOD + SDPCMD_INTSTATUS, cur | SDPCMD_INTSTATUS_HMB_FRAME_IND);
                            self.irq = true;
                        }
                        break;
                    };
                    let n = (front.len() - self.cursor).min(buf.len() - done);
                    buf[done..done + n].copy_from_slice(&front[self.cursor..self.cursor + n]);
                    self.cursor += n;
                    done += n;
                    if self.cursor == front.len() {
                        self.to_host.pop_front();
                        self.cursor = 0;
                        break;
                    }
                }
                Ok(())
            }
            _ => Err(SdioError),
        }
    }

    fn write(&mut self, func: u8, addr: u32, data: &[u8], incr: bool) -> Result<(), SdioError> {
        assert_eq!(data.len() % 4, 0, "whole words");
        assert!(incr);
        match func {
            1 => {
                assert!(addr & SB_ACCESS_2_4B_FLAG != 0, "a backplane access");
                let base = self.bar0 | (addr & SB_OFT_ADDR_MASK);
                for (i, w) in data.chunks(4).enumerate() {
                    self.bp_write32(base + 4 * i as u32, u32::from_le_bytes([w[0], w[1], w[2], w[3]]));
                }
                Ok(())
            }
            2 => {
                assert!(self.f0[0x02] & 4 != 0, "function 2 is enabled");
                self.frame_from_host(data);
                Ok(())
            }
            _ => Err(SdioError),
        }
    }

    fn set_block_size(&mut self, func: u8, size: u16) -> Result<(), SdioError> {
        self.block[usize::from(func)] = size;
        Ok(())
    }

    fn delay_us(&mut self, _us: u32) {}

    fn now_ms(&mut self) -> u64 {
        // Time passes with every ask.
        static TICK: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        TICK.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    }
}
