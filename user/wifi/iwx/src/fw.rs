//! The firmware file: a header, then type-length-value records (from
//! iwx_read_firmware in OpenBSD's if_iwx.c; notices in LICENSE).
//!
//! What the driver needs from it: the runtime image's sections (LMAC,
//! UMAC, then the paging ones, split by separator records), the API and
//! capability bits, the command versions, the PHY SKU and the scan limits.

use alloc::vec::Vec;

const MAGIC: u32 = 0x0a4c_5749;
const HEADER: usize = 88;

const TLV_PROBE_MAX_LEN: u32 = 6;
const TLV_PAN: u32 = 7;
const TLV_FLAGS: u32 = 18;
const TLV_SEC_RT: u32 = 19;
const TLV_PHY_SKU: u32 = 23;
const TLV_NUM_OF_CPU: u32 = 27;
const TLV_API_CHANGES_SET: u32 = 29;
const TLV_ENABLED_CAPABILITIES: u32 = 30;
const TLV_N_SCAN_CHANNELS: u32 = 31;
const TLV_FW_VERSION: u32 = 36;
const TLV_CMD_VERSIONS: u32 = 48;
const TLV_UMAC_DEBUG_ADDRS: u32 = 54;
const TLV_LMAC_DEBUG_ADDRS: u32 = 55;

/// The separators between the LMAC and UMAC sections, and before the
/// paging ones (a section's device offset).
const CPU1_CPU2_SEPARATOR: u32 = 0xFFFF_CCCC;
const PAGING_SEPARATOR: u32 = 0xAAAA_BBBB;

/// The probe request the firmware sends may be this long at most.
pub const PROBE_REQ_MAX: u32 = 512;
/// The scan channels a firmware may list (iwx-cc-a0-62 and later).
pub const MAX_SCAN_CHANNELS: u32 = 67;
const DEFAULT_SCAN_CHANNELS: u32 = 40;
/// The firmware's debug addresses carry cache control bits.
const ADDR_CACHE_CONTROL: u32 = 0xC000_0000;

pub const API_BITS: usize = 128;
pub const CAPA_BITS: usize = 160;

/// One section of the runtime image: where the firmware loads it, and its
/// bytes (in the file).
#[derive(Clone, Copy)]
pub struct Section<'a> {
    pub devoff: u32,
    pub data: &'a [u8],
}

/// A command's version as the firmware implements it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct CmdVersion {
    pub cmd: u8,
    pub group: u8,
    pub cmd_ver: u8,
    pub notif_ver: u8,
}

/// The version the firmware does not list.
pub const VER_UNKNOWN: u8 = 99;

/// What a firmware file holds.
pub struct Firmware<'a> {
    /// `release/core74::563a6e92` and the like.
    pub name: &'a [u8],
    /// major, minor, API (from the header, or the FW_VERSION record).
    pub version: [u32; 3],
    pub lmac: Vec<Section<'a>>,
    pub umac: Vec<Section<'a>>,
    pub paging: Vec<Section<'a>>,
    api: [u32; API_BITS / 32],
    capa: [u32; CAPA_BITS / 32],
    pub cmd_versions: Vec<CmdVersion>,
    /// The radio configuration: transmit chains in bits 16-19, receive in
    /// 20-23.
    pub phy_sku: u32,
    pub flags: u32,
    pub max_probe_len: u32,
    pub n_scan_channels: u32,
    /// Where the firmware keeps its error logs (SRAM addresses).
    pub umac_error_table: u32,
    pub lmac_error_table: u32,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Error {
    /// Not a TLV firmware file.
    Header,
    /// A record runs past the end of the file.
    Truncated,
    /// Record `0` holds what it should not.
    Record(u32),
    /// No runtime image, or not split as the driver expects.
    Sections,
}

impl<'a> Firmware<'a> {
    pub fn parse(file: &'a [u8]) -> Result<Self, Error> {
        if file.len() < HEADER || le32(file, 0) != 0 || le32(file, 4) != MAGIC {
            return Err(Error::Header);
        }
        let ver = le32(file, 72);
        let name = &file[8..72];
        let name = &name[..name.iter().position(|&c| c == 0).unwrap_or(name.len())];
        let mut fw = Firmware {
            name,
            version: [ver >> 24, (ver >> 16) & 0xff, (ver >> 8) & 0xff],
            lmac: Vec::new(),
            umac: Vec::new(),
            paging: Vec::new(),
            api: [0; API_BITS / 32],
            capa: [0; CAPA_BITS / 32],
            cmd_versions: Vec::new(),
            phy_sku: 0,
            flags: 0,
            max_probe_len: 0,
            n_scan_channels: DEFAULT_SCAN_CHANNELS,
            umac_error_table: 0,
            lmac_error_table: 0,
        };
        let mut sections: Vec<Section<'a>> = Vec::new();
        let mut rest = &file[HEADER..];
        while rest.len() >= 8 {
            let (ty, len) = (le32(rest, 0), le32(rest, 4) as usize);
            rest = &rest[8..];
            let data = rest.get(..len).ok_or(Error::Truncated)?;
            fw.record(ty, data, &mut sections)?;
            // The last record may lack its padding.
            let padded = len.next_multiple_of(4);
            if padded > rest.len() {
                break;
            }
            rest = &rest[padded..];
        }
        fw.split(&sections)?;
        Ok(fw)
    }

    fn record(&mut self, ty: u32, d: &'a [u8], sections: &mut Vec<Section<'a>>) -> Result<(), Error> {
        let bad = || Error::Record(ty);
        let u32_only = || if d.len() == 4 { Ok(le32(d, 0)) } else { Err(bad()) };
        match ty {
            TLV_PROBE_MAX_LEN => {
                self.max_probe_len = le32(d.get(..4).ok_or(bad())?, 0);
                if self.max_probe_len > PROBE_REQ_MAX {
                    return Err(bad());
                }
            }
            TLV_PAN if !d.is_empty() => return Err(bad()),
            TLV_FLAGS => self.flags = le32(d.get(..4).ok_or(bad())?, 0),
            TLV_SEC_RT => {
                if d.len() < 4 {
                    return Err(bad());
                }
                sections.push(Section { devoff: le32(d, 0), data: &d[4..] });
            }
            TLV_PHY_SKU => self.phy_sku = u32_only()?,
            TLV_NUM_OF_CPU if !matches!(u32_only()?, 1 | 2) => return Err(bad()),
            TLV_API_CHANGES_SET | TLV_ENABLED_CAPABILITIES => {
                if d.len() != 8 {
                    return Err(bad());
                }
                let (idx, bits) = (le32(d, 0) as usize, le32(d, 4));
                let set: &mut [u32] = if ty == TLV_API_CHANGES_SET { &mut self.api } else { &mut self.capa };
                *set.get_mut(idx).ok_or(bad())? |= bits;
            }
            TLV_N_SCAN_CHANNELS => {
                self.n_scan_channels = u32_only()?;
                if self.n_scan_channels > MAX_SCAN_CHANNELS {
                    return Err(bad());
                }
            }
            TLV_FW_VERSION => {
                if d.len() != 12 {
                    return Err(bad());
                }
                self.version = [le32(d, 0), le32(d, 4), le32(d, 8)];
            }
            TLV_CMD_VERSIONS => {
                if !self.cmd_versions.is_empty() {
                    return Err(bad());
                }
                self.cmd_versions = d
                    .chunks_exact(4)
                    .map(|c| CmdVersion { cmd: c[0], group: c[1], cmd_ver: c[2], notif_ver: c[3] })
                    .collect();
            }
            // iwx_umac_debug_addrs (2 words) and iwx_lmac_debug_addrs (8):
            // the error log's address first.
            TLV_UMAC_DEBUG_ADDRS if d.len() == 8 => self.umac_error_table = le32(d, 0) & !ADDR_CACHE_CONTROL,
            TLV_LMAC_DEBUG_ADDRS if d.len() == 32 => self.lmac_error_table = le32(d, 0) & !ADDR_CACHE_CONTROL,
            TLV_UMAC_DEBUG_ADDRS | TLV_LMAC_DEBUG_ADDRS => return Err(bad()),
            // The rest (initialization and WoWLAN images, debug records,
            // the ones Intel left undocumented) the driver does not need.
            _ => {}
        }
        Ok(())
    }

    /// LMAC sections, the CPU1/CPU2 separator, UMAC sections, the paging
    /// separator, paging sections (iwx_init_fw_sec).
    fn split(&mut self, s: &[Section<'a>]) -> Result<(), Error> {
        let sep = |x: &Section| x.devoff == CPU1_CPU2_SEPARATOR || x.devoff == PAGING_SEPARATOR;
        let run = |from: usize| s.iter().skip(from).take_while(|x| !sep(x)).count();
        let lmac = run(0);
        let umac = run(lmac + 1);
        let paging = run(lmac + umac + 2);
        if lmac == 0 || umac == 0 || s.get(lmac).map(|x| x.devoff) != Some(CPU1_CPU2_SEPARATOR) {
            return Err(Error::Sections);
        }
        self.lmac = s[..lmac].to_vec();
        self.umac = s[lmac + 1..lmac + 1 + umac].to_vec();
        if paging > 0 {
            self.paging = s[lmac + umac + 2..lmac + umac + 2 + paging].to_vec();
        }
        Ok(())
    }

    pub fn api(&self, bit: usize) -> bool {
        bit < API_BITS && self.api[bit / 32] & (1 << (bit % 32)) != 0
    }

    pub fn capa(&self, bit: usize) -> bool {
        bit < CAPA_BITS && self.capa[bit / 32] & (1 << (bit % 32)) != 0
    }

    /// The version of command `cmd` of `group` (iwx_lookup_cmd_ver).
    pub fn cmd_ver(&self, group: u8, cmd: u8) -> u8 {
        self.cmd_versions.iter().find(|v| v.group == group && v.cmd == cmd).map_or(VER_UNKNOWN, |v| v.cmd_ver)
    }

    /// The version of the notification answering `cmd` of `group`.
    pub fn notif_ver(&self, group: u8, cmd: u8) -> u8 {
        self.cmd_versions.iter().find(|v| v.group == group && v.cmd == cmd).map_or(VER_UNKNOWN, |v| v.notif_ver)
    }

    /// The radio's valid transmit and receive chains (antennas).
    pub fn tx_chains(&self) -> u32 {
        (self.phy_sku >> 16) & 0xf
    }

    pub fn rx_chains(&self) -> u32 {
        (self.phy_sku >> 20) & 0xf
    }
}

pub(crate) fn le32(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]])
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    fn tlv(out: &mut Vec<u8>, ty: u32, data: &[u8]) {
        out.extend_from_slice(&ty.to_le_bytes());
        out.extend_from_slice(&(data.len() as u32).to_le_bytes());
        out.extend_from_slice(data);
        while out.len() % 4 != 0 {
            out.push(0);
        }
    }

    fn section(devoff: u32, body: &[u8]) -> Vec<u8> {
        let mut s = devoff.to_le_bytes().to_vec();
        s.extend_from_slice(body);
        s
    }

    fn header() -> Vec<u8> {
        let mut f = vec![0u8; HEADER];
        f[4..8].copy_from_slice(&MAGIC.to_le_bytes());
        f[8..18].copy_from_slice(b"release/x\0");
        f[72..76].copy_from_slice(&0x4d4d_4d00u32.to_le_bytes());
        f
    }

    #[test]
    fn sections_split_and_records() {
        let mut f = header();
        tlv(&mut f, TLV_SEC_RT, &section(0x0040_0000, b"lmac0"));
        tlv(&mut f, TLV_SEC_RT, &section(0x0040_1000, b"lmac1"));
        tlv(&mut f, TLV_SEC_RT, &section(CPU1_CPU2_SEPARATOR, b""));
        tlv(&mut f, TLV_SEC_RT, &section(0x0080_0000, b"umac0"));
        tlv(&mut f, TLV_SEC_RT, &section(PAGING_SEPARATOR, b""));
        tlv(&mut f, TLV_SEC_RT, &section(0x0, b"page0"));
        tlv(&mut f, TLV_PHY_SKU, &0x0033_0018u32.to_le_bytes());
        tlv(&mut f, TLV_ENABLED_CAPABILITIES, &[0, 0, 0, 0, 0b110, 0, 0, 0]);
        tlv(&mut f, TLV_API_CHANGES_SET, &[1, 0, 0, 0, 0, 0, 0, 1]);
        tlv(&mut f, TLV_CMD_VERSIONS, &[0x0d, 1, 15, 0, 0x01, 0, 0, 6]);
        tlv(&mut f, TLV_N_SCAN_CHANNELS, &67u32.to_le_bytes());
        tlv(&mut f, 0x100000c, b"ignored, odd length");
        let fw = Firmware::parse(&f).unwrap();
        assert_eq!(fw.name, b"release/x");
        assert_eq!(fw.version, [0x4d, 0x4d, 0x4d]);
        assert_eq!(fw.lmac.len(), 2);
        assert_eq!(fw.lmac[1].devoff, 0x0040_1000);
        assert_eq!(fw.lmac[1].data, b"lmac1");
        assert_eq!(fw.umac.len(), 1);
        assert_eq!(fw.paging.len(), 1);
        assert_eq!(fw.paging[0].data, b"page0");
        assert_eq!((fw.tx_chains(), fw.rx_chains()), (3, 3));
        assert!(fw.capa(1) && fw.capa(2) && !fw.capa(0) && !fw.capa(12));
        assert!(fw.api(56) && !fw.api(55));
        assert_eq!(fw.cmd_ver(1, 0x0d), 15);
        assert_eq!(fw.notif_ver(0, 0x01), 6);
        assert_eq!(fw.cmd_ver(1, 0x0c), VER_UNKNOWN);
        assert_eq!(fw.n_scan_channels, 67);
    }

    #[test]
    fn refuses_bad_files() {
        assert_eq!(Firmware::parse(&[0u8; 10]).err(), Some(Error::Header));
        let mut f = header();
        f[4] ^= 1;
        assert_eq!(Firmware::parse(&f).err(), Some(Error::Header));
        // A record longer than the file.
        let mut f = header();
        f.extend_from_slice(&TLV_SEC_RT.to_le_bytes());
        f.extend_from_slice(&100u32.to_le_bytes());
        f.extend_from_slice(&[0; 8]);
        assert_eq!(Firmware::parse(&f).err(), Some(Error::Truncated));
        // No CPU1/CPU2 separator.
        let mut f = header();
        tlv(&mut f, TLV_SEC_RT, &section(0x0040_0000, b"lmac0"));
        assert_eq!(Firmware::parse(&f).err(), Some(Error::Sections));
        // Too many scan channels.
        let mut f = header();
        tlv(&mut f, TLV_N_SCAN_CHANNELS, &68u32.to_le_bytes());
        assert_eq!(Firmware::parse(&f).err(), Some(Error::Record(TLV_N_SCAN_CHANNELS)));
    }

    /// The pinned AX201 firmware, when `IWX_FIRMWARE` names it
    /// (`wifi-firmware fetch` on the host): what the survey of it found.
    #[test]
    fn pinned_firmware() {
        extern crate std;
        let Ok(path) = std::env::var("IWX_FIRMWARE") else {
            return;
        };
        let file = std::fs::read(path).unwrap();
        let fw = Firmware::parse(&file).unwrap();
        assert_eq!(fw.name, b"release/core74::563a6e92");
        assert_eq!((fw.lmac.len(), fw.umac.len(), fw.paging.len()), (14, 15, 20));
        assert_eq!(fw.lmac.iter().map(|s| s.data.len()).sum::<usize>(), 375_756);
        assert_eq!(fw.umac.iter().map(|s| s.data.len()).sum::<usize>(), 393_020);
        assert_eq!(fw.paging.iter().map(|s| s.data.len()).sum::<usize>(), 620_152);
        assert_eq!((fw.tx_chains(), fw.rx_chains()), (3, 3));
        assert_eq!((fw.max_probe_len, fw.n_scan_channels), (512, 67));
        assert_eq!(fw.cmd_ver(1, 0x0d), 15);
        assert_eq!(fw.notif_ver(0, 0x01), 6);
        assert!(fw.capa(1) && fw.capa(2) && fw.capa(9) && !fw.capa(12));
        assert!(fw.api(56) && fw.api(58) && !fw.api(68));
    }
}
