//! The chip's registers, protocol constants and structure layouts, from
//! OpenBSD's bwfmreg.h and if_bwfm_sdio.h: the SDIO function 1 registers,
//! the backplane (chipcommon, the AI agents, the ARM cores, the SDIO
//! device core), the SDPCM and BCDC headers, the firmware's commands,
//! variables and events.

// ---- SDIO function 0 (Broadcom's CCCR extension) and function 1 --------

pub const CCCR_CARDCAP: u32 = 0xf0;
pub const CCCR_CARDCAP_CMD14_SUPPORT: u8 = 1 << 1;
pub const CCCR_CARDCAP_CMD14_EXT: u8 = 1 << 2;
pub const CCCR_CARDCTRL: u32 = 0xf1;
pub const CCCR_CARDCTRL_WLANRESET: u8 = 1 << 1;

pub const SDIO_WATERMARK: u32 = 0x10008;
pub const SDIO_DEVICE_CTL: u32 = 0x10009;
pub const SDIO_DEVICE_CTL_CA_INT_ONLY: u8 = 0x04;
pub const FUNC1_SBADDRLOW: u32 = 0x1000a;
pub const FUNC1_SBADDRMID: u32 = 0x1000b;
pub const FUNC1_SBADDRHIGH: u32 = 0x1000c;
pub const FUNC1_CHIPCLKCSR: u32 = 0x1000e;
pub const CHIPCLKCSR_FORCE_ALP: u8 = 0x01;
pub const CHIPCLKCSR_FORCE_HT: u8 = 0x02;
pub const CHIPCLKCSR_ALP_AVAIL_REQ: u8 = 0x08;
pub const CHIPCLKCSR_HT_AVAIL_REQ: u8 = 0x10;
pub const CHIPCLKCSR_FORCE_HW_CLKREQ_OFF: u8 = 0x20;
pub const CHIPCLKCSR_ALP_AVAIL: u8 = 0x40;
pub const CHIPCLKCSR_HT_AVAIL: u8 = 0x80;
pub const CHIPCLKCSR_AVBITS: u8 = CHIPCLKCSR_ALP_AVAIL | CHIPCLKCSR_HT_AVAIL;
pub const FUNC1_SDIOPULLUP: u32 = 0x1000f;
pub const FUNC1_WAKEUPCTRL: u32 = 0x1001e;
pub const WAKEUPCTRL_HTWAIT: u8 = 1 << 1;
pub const FUNC1_SLEEPCSR: u32 = 0x1001f;
pub const SLEEPCSR_KSO: u8 = 1 << 0;

/// The backplane window: function 1 addresses `0x8000` and up reach the
/// 32 KiB page the SBADDR registers select, 32-bit wide with the flag.
pub const SB_OFT_ADDR_PAGE: u32 = 0x8000;
pub const SB_OFT_ADDR_MASK: u32 = 0x7fff;
pub const SB_ACCESS_2_4B_FLAG: u32 = 0x8000;

/// ALP (or HT) is available; `alp_only`: ALP is enough.
pub fn clock_available(csr: u8, alp_only: bool) -> bool {
    csr & CHIPCLKCSR_AVBITS != 0 && (alp_only || csr & CHIPCLKCSR_AVBITS == CHIPCLKCSR_AVBITS)
}

// ---- Chipcommon and the AI interconnect ---------------------------------

pub const CHIP_BASE: u32 = 0x1800_0000;
pub const CHIP_REG_CHIPID: u32 = 0x0000;
pub const CHIP_REG_CAPABILITIES: u32 = 0x0004;
pub const CHIP_REG_CAPABILITIES_PMU: u32 = 0x1000_0000;
pub const CHIP_REG_CAPABILITIES_EXT: u32 = 0x00ac;
pub const CHIP_REG_CAPABILITIES_EXT_AOB_PRESENT: u32 = 0x40;
pub const CHIP_REG_EROMPTR: u32 = 0x00fc;
pub const CHIP_REG_SR_CONTROL0: u32 = 0x0504;
pub const CHIP_REG_SR_CONTROL0_ENABLE: u32 = 1;
pub const CHIP_REG_SR_CONTROL1: u32 = 0x0508;
pub const CHIP_REG_PMUCONTROL: u32 = 0x0600;
pub const CHIP_REG_PMUCONTROL_RES_SHIFT: u32 = 13;
pub const CHIP_REG_PMUCONTROL_RES_RELOAD: u32 = 0x2;
pub const CHIP_REG_PMUCAPABILITIES: u32 = 0x0604;
pub const CHIP_REG_PMUCAPABILITIES_REV_MASK: u32 = 0xff;
pub const CHIP_REG_PMUCAPABILITIES_EXT: u32 = 0x064c;
pub const CHIP_REG_PMUCAPABILITIES_SR_SUPP: u32 = 1 << 1;
pub const CHIP_REG_CHIPCONTROL_ADDR: u32 = 0x0650;
pub const CHIP_REG_CHIPCONTROL_DATA: u32 = 0x0654;
pub const CHIP_REG_RETENTION_CTL: u32 = 0x0670;
pub const CHIP_REG_RETENTION_CTL_MACPHY_DIS: u32 = 1 << 26;
pub const CHIP_REG_RETENTION_CTL_LOGIC_DIS: u32 = 1 << 27;

/// The chip id register's fields.
pub fn chipid_id(v: u32) -> u32 {
    v & 0xffff
}
pub fn chipid_rev(v: u32) -> u32 {
    (v >> 16) & 0xf
}
pub fn chipid_type(v: u32) -> u32 {
    (v >> 28) & 0xf
}
pub const CHIPID_TYPE_SOCI_SB: u32 = 0;
pub const CHIPID_TYPE_SOCI_AI: u32 = 1;

/// An AI agent's (core wrapper's) registers.
pub const AGENT_IOCTL: u32 = 0x0408;
pub const AGENT_IOCTL_CLK: u32 = 0x0001;
pub const AGENT_IOCTL_FGC: u32 = 0x0002;
pub const AGENT_IOCTL_ARMCR4_CPUHALT: u32 = 0x0020;
pub const AGENT_RESET_CTL: u32 = 0x0800;
pub const AGENT_RESET_CTL_RESET: u32 = 0x0001;
pub const AGENT_D11_IOCTL_PHYCLOCKEN: u32 = 0x0004;
pub const AGENT_D11_IOCTL_PHYRESET: u32 = 0x0008;

/// Core ids.
pub const CORE_CHIPCOMMON: u16 = 0x800;
pub const CORE_INTERNAL_MEM: u16 = 0x80e;
pub const CORE_80211: u16 = 0x812;
pub const CORE_PMU: u16 = 0x827;
pub const CORE_SDIO_DEV: u16 = 0x829;
pub const CORE_ARM_CM3: u16 = 0x82a;
pub const CORE_ARM_CR4: u16 = 0x83e;
pub const CORE_GCI: u16 = 0x840;
pub const CORE_ARM_CA7: u16 = 0x847;
pub const CORE_SYS_MEM: u16 = 0x849;

/// The ARM CR4's tightly coupled memory banks.
pub const ARMCR4_CAP: u32 = 0x0004;
pub const ARMCR4_CAP_TCBANB_MASK: u32 = 0xf;
pub const ARMCR4_CAP_TCBBNB_MASK: u32 = 0xf0;
pub const ARMCR4_CAP_TCBBNB_SHIFT: u32 = 4;
pub const ARMCR4_BANKIDX: u32 = 0x0040;
pub const ARMCR4_BANKINFO: u32 = 0x0044;
pub const ARMCR4_BANKINFO_BSZ_MASK: u32 = 0x7f;
pub const ARMCR4_BANKINFO_BLK_1K_MASK: u32 = 0x200;

/// The SOCRAM core.
pub const SOCRAM_COREINFO: u32 = 0x0000;
pub const SOCRAM_COREINFO_SRBSZ_BASE: u32 = 14;
pub const SOCRAM_COREINFO_SRBSZ_MASK: u32 = 0xf;
pub const SOCRAM_COREINFO_SRNB_MASK: u32 = 0xf0;
pub const SOCRAM_COREINFO_SRNB_SHIFT: u32 = 4;
pub const SOCRAM_COREINFO_LSS_MASK: u32 = 0xf0_0000;
pub const SOCRAM_COREINFO_LSS_SHIFT: u32 = 20;
pub const SOCRAM_BANKIDX: u32 = 0x0010;
pub const SOCRAM_BANKIDX_MEMTYPE_RAM: u32 = 0;
pub const SOCRAM_BANKIDX_MEMTYPE_SHIFT: u32 = 8;
pub const SOCRAM_BANKINFO: u32 = 0x0040;
pub const SOCRAM_BANKINFO_SZBASE: u32 = 8192;
pub const SOCRAM_BANKINFO_SZMASK: u32 = 0x7f;
pub const SOCRAM_BANKINFO_RETNTRAM_MASK: u32 = 0x1_0000;
pub const SOCRAM_BANKPDA: u32 = 0x0044;

/// The DMP (enumeration ROM) descriptors.
pub const DMP_DESC_MASK: u32 = 0xf;
pub const DMP_DESC_COMPONENT: u32 = 0x1;
pub const DMP_DESC_MASTER_PORT: u32 = 0x3;
pub const DMP_DESC_ADDRESS: u32 = 0x5;
pub const DMP_DESC_ADDRSIZE_GT32: u32 = 0x8;
pub const DMP_DESC_EOT: u32 = 0xf;
pub const DMP_COMP_PARTNUM: u32 = 0x000f_ff00;
pub const DMP_COMP_PARTNUM_S: u32 = 8;
pub const DMP_COMP_REVISION: u32 = 0xff00_0000;
pub const DMP_COMP_REVISION_S: u32 = 24;
pub const DMP_COMP_NUM_SWRAP: u32 = 0x00f8_0000;
pub const DMP_COMP_NUM_SWRAP_S: u32 = 19;
pub const DMP_COMP_NUM_MWRAP: u32 = 0x0007_c000;
pub const DMP_COMP_NUM_MWRAP_S: u32 = 14;
pub const DMP_MASTER_PORT_NUM: u32 = 0xf0;
pub const DMP_MASTER_PORT_NUM_S: u32 = 4;
pub const DMP_SLAVE_ADDR_BASE: u32 = 0xffff_f000;
pub const DMP_SLAVE_TYPE: u32 = 0xc0;
pub const DMP_SLAVE_TYPE_S: u32 = 6;
pub const DMP_SLAVE_TYPE_SLAVE: u32 = 0;
pub const DMP_SLAVE_TYPE_SWRAP: u32 = 2;
pub const DMP_SLAVE_TYPE_MWRAP: u32 = 3;
pub const DMP_SLAVE_SIZE_TYPE: u32 = 0x30;
pub const DMP_SLAVE_SIZE_TYPE_S: u32 = 4;
pub const DMP_SLAVE_SIZE_4K: u32 = 0;
pub const DMP_SLAVE_SIZE_8K: u32 = 1;
pub const DMP_SLAVE_SIZE_DESC: u32 = 3;

// ---- The SDIO device core (SDPCM) ----------------------------------------

pub const SDPCMD_INTSTATUS: u32 = 0x020;
pub const SDPCMD_INTSTATUS_HMB_SW_MASK: u32 = 0xf0;
pub const SDPCMD_INTSTATUS_HMB_FRAME_IND: u32 = 1 << 6;
pub const SDPCMD_INTSTATUS_HMB_HOST_INT: u32 = 1 << 7;
pub const SDPCMD_INTSTATUS_CHIPACTIVE: u32 = 1 << 29;
pub const SDPCMD_HOSTINTMASK: u32 = 0x024;
pub const SDPCMD_TOSBMAILBOX: u32 = 0x040;
pub const SDPCMD_TOSBMAILBOX_INT_ACK: u32 = 1 << 1;
pub const SDPCMD_TOSBMAILBOXDATA: u32 = 0x048;
pub const SDPCMD_TOHOSTMAILBOXDATA: u32 = 0x04c;
pub const SDPCMD_TOHOSTMAILBOXDATA_NAKHANDLED: u32 = 1 << 0;
pub const SDPCMD_TOHOSTMAILBOXDATA_DEVREADY: u32 = 1 << 1;
pub const SDPCMD_TOHOSTMAILBOXDATA_FWREADY: u32 = 1 << 3;
pub const SDPCM_PROT_VERSION: u32 = 4;
pub const SDPCM_PROT_VERSION_SHIFT: u32 = 16;

/// An SDPCM frame: the hardware header (length, its complement), the
/// software header (sequence, channel, next frame's length in 16-byte
/// units, data offset, flow control, the sequence the host may send up
/// to), then the payload.
pub const SDPCM_HWHDR_LEN: usize = 4;
pub const SDPCM_SWHDR_LEN: usize = 8;
pub const SDPCM_HDR_LEN: usize = SDPCM_HWHDR_LEN + SDPCM_SWHDR_LEN;
pub const SDPCM_CHANNEL_CONTROL: u8 = 0;
pub const SDPCM_CHANNEL_EVENT: u8 = 1;
pub const SDPCM_CHANNEL_DATA: u8 = 2;
pub const SDPCM_CHANNEL_GLOM: u8 = 3;
pub const SDPCM_CHANNEL_MASK: u8 = 0x0f;

/// The shared structure the firmware leaves at the end of RAM: its
/// console's address is at offset 20.
pub const SDPCM_SHARED_CONSOLE_ADDR: usize = 20;

// ---- BCDC ------------------------------------------------------------------

/// A control message's header: command, length, flags, status (32 bits
/// each, little endian).
pub const BCDC_DCMD_HDR_LEN: usize = 16;
pub const BCDC_DCMD_ERROR: u32 = 1 << 0;
pub const BCDC_DCMD_SET: u32 = 1 << 1;
pub const BCDC_DCMD_ID_SHIFT: u32 = 16;
pub const BCDC_DCMD_IF_SHIFT: u32 = 12;
/// A frame's header: flags (the protocol version in the upper nibble),
/// priority, flags2 (the interface), data offset in words.
pub const BCDC_HDR_LEN: usize = 4;
pub const BCDC_FLAG_PROTO_VER: u8 = 2;
/// The most a command carries.
pub const BCDC_BUF_MAX: usize = 8192;

// ---- The firmware's commands, variables and parameters ------------------

pub const C_GET_VERSION: u32 = 1;
pub const C_UP: u32 = 2;
pub const C_DOWN: u32 = 3;
pub const C_SET_INFRA: u32 = 20;
pub const C_GET_BSSID: u32 = 23;
pub const C_SET_SSID: u32 = 26;
pub const C_DISASSOC: u32 = 52;
pub const C_SET_PM: u32 = 86;
pub const C_GET_RSSI: u32 = 127;
pub const C_SET_AP: u32 = 118;
pub const C_GET_BANDLIST: u32 = 140;
pub const C_SET_SCAN_CHANNEL_TIME: u32 = 185;
pub const C_SET_SCAN_UNASSOC_TIME: u32 = 187;
pub const C_SET_SCAN_PASSIVE_TIME: u32 = 258;
pub const C_GET_VAR: u32 = 262;
pub const C_SET_VAR: u32 = 263;
pub const C_SET_WSEC_PMK: u32 = 268;

pub const IO_TYPE_D11N: u32 = 1;
pub const IO_TYPE_D11AC: u32 = 2;
pub const BAND_2G: u32 = 2;
pub const BAND_5G: u32 = 1;
pub const PM_FAST_PS: u32 = 2;
pub const DEFAULT_SCAN_CHANNEL_TIME: u32 = 40;
pub const DEFAULT_SCAN_UNASSOC_TIME: u32 = 40;
pub const DEFAULT_SCAN_PASSIVE_TIME: u32 = 120;

pub const AUTH_OPEN: u32 = 0;
pub const MFP_NONE: u32 = 0;
pub const WPA_AUTH_DISABLED: u32 = 0;
pub const WPA_AUTH_WPA2_PSK: u32 = 1 << 7;
pub const WSEC_NONE: u32 = 0;
pub const WSEC_AES: u32 = 1 << 2;
pub const WSEC_PASSPHRASE: u16 = 1 << 0;
pub const WSEC_MAX_PSK_LEN: usize = 32;

/// Channel specifications (D11N and D11AC encodings).
pub const CHANSPEC_CHAN_MASK: u32 = 0xff;
pub const CHANSPEC_D11N_SB_L: u32 = 0x1 << 8;
pub const CHANSPEC_D11N_SB_U: u32 = 0x2 << 8;
pub const CHANSPEC_D11N_SB_N: u32 = 0x3 << 8;
pub const CHANSPEC_D11N_SB_MASK: u32 = 0x3 << 8;
pub const CHANSPEC_D11N_BW_20: u32 = 0x2 << 10;
pub const CHANSPEC_D11N_BW_40: u32 = 0x3 << 10;
pub const CHANSPEC_D11N_BW_MASK: u32 = 0x3 << 10;
pub const CHANSPEC_D11N_BND_2G: u32 = 0x2 << 12;
pub const CHANSPEC_D11AC_SB_LLL: u32 = 0x0 << 8;
pub const CHANSPEC_D11AC_SB_LLU: u32 = 0x1 << 8;
pub const CHANSPEC_D11AC_SB_LUL: u32 = 0x2 << 8;
pub const CHANSPEC_D11AC_SB_LUU: u32 = 0x3 << 8;
pub const CHANSPEC_D11AC_SB_MASK: u32 = 0x7 << 8;
pub const CHANSPEC_D11AC_BW_20: u32 = 0x2 << 11;
pub const CHANSPEC_D11AC_BW_40: u32 = 0x3 << 11;
pub const CHANSPEC_D11AC_BW_80: u32 = 0x4 << 11;
pub const CHANSPEC_D11AC_BW_MASK: u32 = 0x7 << 11;
pub const CHANSPEC_D11AC_BND_2G: u32 = 0x0 << 14;

/// Scanning: the escan request and its results.
pub const ESCAN_REQ_VERSION: u32 = 1;
pub const ESCAN_REQ_VERSION_V2: u32 = 2;
pub const ESCAN_ACTION_START: u16 = 1;
pub const ESCAN_ACTION_ABORT: u16 = 3;
pub const SCANTYPE_ACTIVE: u8 = 0;
pub const SCANTYPE_PASSIVE: u8 = 1;
pub const BSSTYPE_ANY: u8 = 2;
pub const MAX_SSID_LEN: usize = 32;
/// `struct bwfm_ssid`: a 32-bit length and 32 bytes.
pub const SSID_LEN: usize = 36;
/// `struct bwfm_bss_info`'s fixed part (128 bytes, padded) and its
/// fields' offsets.
pub const BSS_INFO_LEN: usize = 128;
pub const BSS_INFO_LENGTH: usize = 4;
pub const BSS_INFO_BSSID: usize = 8;
pub const BSS_INFO_BEACON_PERIOD: usize = 14;
pub const BSS_INFO_CAPABILITY: usize = 16;
pub const BSS_INFO_SSID_LEN: usize = 18;
pub const BSS_INFO_SSID: usize = 19;
pub const BSS_INFO_CHANSPEC: usize = 72;
pub const BSS_INFO_RSSI: usize = 78;
pub const BSS_INFO_CTL_CH: usize = 88;
pub const BSS_INFO_IE_OFFSET: usize = 116;
pub const BSS_INFO_IE_LENGTH: usize = 120;
/// The escan results header: buflen, version, sync_id, bss_count.
pub const ESCAN_RESULTS_LEN: usize = 12;

/// Events: the firmware's message after a Broadcom Ethernet header
/// (EtherType `0x886c`, subtype, length, version, OUI, user subtype 1),
/// then its message: version, flags, event type, status, reason, auth
/// type, data length (all big endian), the address, an interface name,
/// the interface's index and bsscfg index; then the event's data.
pub const ETHERTYPE_LINK_CTL: u16 = 0x886c;
pub const BRCM_OUI: [u8; 3] = [0x00, 0x10, 0x18];
pub const BRCM_SUBTYPE_EVENT: u16 = 1;
pub const EVENT_ETHHDR_LEN: usize = 10;
pub const EVENT_MSG_LEN: usize = 48;
pub const EVENT_LEN: usize = 14 + EVENT_ETHHDR_LEN + EVENT_MSG_LEN;
pub const E_SET_SSID: u32 = 0;
pub const E_AUTH: u32 = 3;
pub const E_DEAUTH: u32 = 5;
pub const E_DEAUTH_IND: u32 = 6;
pub const E_ASSOC: u32 = 7;
pub const E_DISASSOC: u32 = 11;
pub const E_DISASSOC_IND: u32 = 12;
pub const E_LINK: u32 = 16;
pub const E_EAPOL_MSG: u32 = 25;
pub const E_PSK_SUP: u32 = 46;
pub const E_IF: u32 = 54;
pub const E_ESCAN_RESULT: u32 = 69;
pub const E_LAST: u32 = 139;
pub const EVENT_MASK_LEN: usize = (E_LAST as usize).div_ceil(8);
pub const E_STATUS_SUCCESS: u32 = 0;
pub const E_STATUS_FAIL: u32 = 1;
pub const E_STATUS_TIMEOUT: u32 = 2;
pub const E_STATUS_NO_NETWORKS: u32 = 3;
pub const E_STATUS_ABORT: u32 = 4;
pub const E_STATUS_PARTIAL: u32 = 8;
/// A LINK event's flag: the link is up.
pub const EVENT_FLAG_LINK_UP: u16 = 1;
/// A PSK_SUP event's status once the firmware's supplicant has its keys.
pub const WLC_SUP_KEYED: u32 = 6;

/// A blob (the CLM) downloaded in `dload_data` pieces: flag, type, length,
/// CRC, then the data.
pub const DLOAD_HDR_LEN: usize = 12;
pub const DLOAD_FLAG_BEGIN: u16 = 1 << 1;
pub const DLOAD_FLAG_END: u16 = 1 << 2;
pub const DLOAD_FLAG_HANDLER_VER_1: u16 = 1 << 12;
pub const DLOAD_TYPE_CLM: u16 = 2;
pub const DLOAD_MAX_LEN: usize = 1400;
