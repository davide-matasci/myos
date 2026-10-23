//! The device's registers (reached through vendor requests), its vendor
//! requests, the eFUSE ROM's words and the descriptors' flags, from
//! OpenBSD's mtwreg.h and if_mtwvar.h (notices in LICENSE). Only what the
//! MT7601U uses.

// Vendor requests (bRequest).
pub const REQ_RESET: u8 = 0x1;
pub const REQ_WRITE_2: u8 = 0x2;
pub const REQ_WRITE_REGION_1: u8 = 0x6;
pub const REQ_READ_REGION_1: u8 = 0x7;
pub const REQ_WRITE_CFG: u8 = 0x46;
pub const REQ_READ_CFG: u8 = 0x47;

// System registers.
pub const ASIC_VER: u16 = 0x0000;
pub const CMB_CTRL: u16 = 0x0020;
pub const EFUSE_CTRL: u16 = 0x0024;
pub const EFUSE_DATA0: u16 = 0x0028;
pub const OSC_CTRL: u16 = 0x0038;
pub const LDO_CFG0: u16 = 0x006c;
pub const WLAN_CTRL: u16 = 0x0080;

pub const EFSROM_KICK: u32 = 1 << 30;
pub const EFSROM_AIN_MASK: u32 = 0x03ff_0000;
pub const EFSROM_AIN_SHIFT: u32 = 16;
pub const EFSROM_MODE_MASK: u32 = 0x0000_00c0;
pub const EFUSE_AOUT_MASK: u32 = 0x0000_003f;
pub const OSC_EN: u32 = 1 << 31;
pub const OSC_CAL_REQ: u32 = 1 << 30;
pub const WLAN_CLK_EN: u32 = 1 << 1;
pub const WLAN_EN: u32 = 1 << 0;

// SCH/DMA registers.
pub const WPDMA_GLO_CFG: u16 = 0x0208;
pub const MCU_DMA_ADDR: u16 = 0x0230;
pub const MCU_DMA_LEN: u16 = 0x0234;
pub const USB_DMA_CFG: u16 = 0x0238;
pub const US_CYC_CNT: u16 = 0x02a4;

pub const TX_DMA_EN: u32 = 1 << 0;
pub const TX_DMA_BUSY: u32 = 1 << 1;
pub const RX_DMA_EN: u32 = 1 << 2;
pub const RX_DMA_BUSY: u32 = 1 << 3;
pub const TX_WB_DDONE: u32 = 1 << 6;
/// MCU_DMA_ADDR, read as a config register: the firmware is running.
pub const MCU_READY: u32 = 1 << 0;

pub const USB_TX_EN: u32 = 1 << 23;
pub const USB_RX_EN: u32 = 1 << 22;
pub const USB_RX_AGG_EN: u32 = 1 << 21;
pub const USB_TXOP_HALT: u32 = 1 << 20;
pub const USB_TX_CLEAR: u32 = 1 << 19;
pub const USB_RX_WL_DROP: u32 = 1 << 25;
/// Aggregation limit, in KiB.
pub const fn usb_rx_agg_lmt(x: u32) -> u32 {
    x << 8
}
/// Aggregation timeout, in 33 ns units.
pub const fn usb_rx_agg_to(x: u32) -> u32 {
    x & 0xff
}

// Packet buffer registers.
pub const PBF_CFG: u16 = 0x0404;
pub const TXRXQ_PCNT: u16 = 0x0438;
pub const TX2Q_PCNT_MASK: u32 = 0x00ff_0000;

// RF registers.
pub const RF_CSR: u16 = 0x0500;
pub const RF_BYPASS0: u16 = 0x0504;
pub const RF_SETTING0: u16 = 0x050c;
pub const RF_CSR_KICK: u32 = 1 << 31;
pub const RF_CSR_WRITE: u32 = 1 << 30;
pub const BANK_SHIFT: u32 = 14;

// MCU registers.
pub const FCE_PSE_CTRL: u16 = 0x0800;
pub const FCE_L2_STUFF: u16 = 0x080c;
pub const TX_CPU_FCE_BASE: u16 = 0x09a0;
pub const TX_CPU_FCE_MAX_COUNT: u16 = 0x09a4;
pub const MCU_FW_IDX: u16 = 0x09a8;
pub const FCE_PDMA: u16 = 0x09c4;
pub const FCE_SKIP_FS: u16 = 0x0a6c;
pub const L2S_WR_MPDU_LEN_EN: u32 = 1 << 4;

// MAC registers.
pub const MAC_VER_ID: u16 = 0x1000;
pub const MAC_SYS_CTRL: u16 = 0x1004;
pub const MAC_ADDR_DW0: u16 = 0x1008;
pub const MAC_ADDR_DW1: u16 = 0x100c;
pub const MAC_BSSID_DW0: u16 = 0x1010;
pub const MAC_BSSID_DW1: u16 = 0x1014;
pub const BBP_CSR: u16 = 0x101c;
pub const BKOFF_SLOT_CFG: u16 = 0x1104;
pub const BCN_TIME_CFG: u16 = 0x1114;
pub const MAC_STATUS_REG: u16 = 0x1200;
pub const TX_PWR_CFG0: u16 = 0x1314;
pub const TX_BAND_CFG: u16 = 0x132c;
pub const TXOP_CTRL_CFG: u16 = 0x1340;
pub const TX_RTS_CFG: u16 = 0x1344;
pub const LG_FBK_CFG0: u16 = 0x135c;
pub const LG_FBK_CFG1: u16 = 0x1360;
pub const TX_ALC_CFG0: u16 = 0x13b0;
pub const TX_PWR_CFG7: u16 = 0x13d4;
pub const TX_PWR_CFG9: u16 = 0x13dc;
pub const RX_FILTR_CFG: u16 = 0x1400;
pub const AUTO_RSP_CFG: u16 = 0x1404;
pub const LEGACY_BASIC_RATE: u16 = 0x1408;
pub const WCID_ENTRY0: u16 = 0x1800;
pub const H2M_MAILBOX: u16 = 0x7010;
pub const IVEIV0: u16 = 0xa000;
pub const WCID_ATTR0: u16 = 0xa800;
pub const SKEY0: u16 = 0xac00;
pub const SKEY_MODE_0_7: u16 = 0xb000;
pub const SKEY_MODE_8_15: u16 = 0xb004;

pub const MAC_SRST: u32 = 1 << 0;
pub const BBP_HRST: u32 = 1 << 1;
pub const MAC_TX_EN: u32 = 1 << 2;
pub const MAC_RX_EN: u32 = 1 << 3;
pub const BBP_CSR_KICK: u32 = 1 << 17;
pub const BBP_CSR_READ: u32 = 1 << 16;
pub const BBP_ADDR_SHIFT: u32 = 8;
pub const TX_STATUS_BUSY: u32 = 1 << 0;
pub const RX_STATUS_BUSY: u32 = 1 << 1;
pub const TSF_TIMER_EN: u32 = 1 << 16;
pub const TBTT_TIMER_EN: u32 = 1 << 19;
pub const BCN_TX_EN: u32 = 1 << 20;
pub const TX_BAND_SEL_2G: u32 = 1 << 2;
pub const TX_BAND_SEL_5G: u32 = 1 << 1;
pub const TX_BAND_UPPER_40M: u32 = 1 << 0;
pub const RTS_THRES_SHIFT: u32 = 8;
pub const CCK_SHORT_EN: u32 = 1 << 4;

// RX_FILTR_CFG: what the MAC drops.
pub const DROP_CRC_ERR: u32 = 1 << 0;
pub const DROP_PHY_ERR: u32 = 1 << 1;
pub const DROP_UC_NOME: u32 = 1 << 2;
pub const DROP_VER_ERR: u32 = 1 << 4;
pub const DROP_DUPL: u32 = 1 << 7;
pub const DROP_CFACK: u32 = 1 << 8;
pub const DROP_CFEND: u32 = 1 << 9;
pub const DROP_ACK: u32 = 1 << 10;
pub const DROP_CTS: u32 = 1 << 11;
pub const DROP_RTS: u32 = 1 << 12;
pub const DROP_PSPOLL: u32 = 1 << 13;
pub const DROP_BA: u32 = 1 << 14;
pub const DROP_CTRL_RSV: u32 = 1 << 16;

/// The station's own table entry (wireless client id) for association
/// id `aid` (MTW_AID2WCID).
pub const fn aid_to_wcid(aid: u16) -> u8 {
    1 + (aid & 7) as u8
}

// The eFUSE ROM's words.
pub const EEPROM_CHIPID: u16 = 0x00;
pub const EEPROM_MAC01: u16 = 0x02;
pub const EEPROM_MAC23: u16 = 0x03;
pub const EEPROM_MAC45: u16 = 0x04;
pub const EEPROM_ANTENNA: u16 = 0x1a;
pub const EEPROM_FREQ_OFFSET: u16 = 0x1d;
pub const EEPROM_LNA: u16 = 0x22;
pub const EEPROM_RSSI1_2GHZ: u16 = 0x23;
pub const EEPROM_PWR2GHZ_BASE1: u16 = 0x29;
pub const EEPROM_RPWR: u16 = 0x6f;

// MCU commands (mtw_mcu_cmd's), and the LED modes.
pub const MCU_CMD_LED: u8 = 16;
pub const MCU_CMD_RADIO: u8 = 20;
pub const MCU_CMD_CALIBRATE: u8 = 31;
pub const LED_MODE_ON: u32 = 0;
pub const LED_MODE_BLINK_TX: u32 = 2;

pub const RF_7601: u16 = 0x7601;
pub const R47_TEMP: u8 = 1 << 2;

// The descriptors: TXD (before a TXWI or an MCU command), TXWI, RXWI.
pub const TXD_CMD: u16 = 1 << 14;
pub const TXD_DATA: u16 = 0 << 14;
pub const TXD_MCU: u16 = 2 << 11;
pub const TXD_WLAN: u16 = 0 << 11;
pub const TXD_QSEL_EDCA: u16 = 2 << 9;
/// The frame carries its own IV: the MAC encrypts nothing.
pub const TXD_WIV: u16 = 1 << 8;
pub const TXD_CMD_SHIFT: u16 = 4;
pub const TXD_80211: u16 = 1 << 3;
pub const TXD_LEN: usize = 4;
pub const TXWI_LEN: usize = 20;
pub const RXWI_LEN: usize = 28;
/// The zeros after a transfer's last descriptor.
pub const DMA_PAD: usize = 4;

pub const TX_TXOP_BACKOFF: u8 = 3;
pub const TX_NSEQ: u8 = 1 << 1;
pub const TX_ACK: u8 = 1 << 0;
pub const PHY_SHIFT: u16 = 14;
pub const PHY_CCK: u16 = 0;
pub const PHY_OFDM: u16 = 1;
pub const PHY_SHPRE: u16 = 1 << 3;

pub const RXD_LEN: u32 = 0x3fff;
pub const RX_DEC: u32 = 1 << 16;
pub const RX_L2PAD: u32 = 1 << 14;
pub const RX_MICERR: u32 = 1 << 10;
pub const RX_ICVERR: u32 = 1 << 9;
pub const RX_CRCERR: u32 = 1 << 8;

/// A bulk IN transfer at most (MTW_MAX_RXSZ).
pub const MAX_RXSZ: usize = 4096;
/// A firmware chunk per bulk transfer.
pub const FW_BLOCK: usize = 0x2000;
/// The firmware's interrupt vector block (after its header).
pub const MCU_IVB_LEN: usize = 0x40;

/// The legacy rates (rt2860_rates): rate in 500 kb/s, the PHY's code,
/// OFDM or CCK, the rate a control response to it goes at, and the ACK's
/// duration (short, long preamble).
pub struct Rate {
    pub rate: u8,
    pub mcs: u8,
    pub ofdm: bool,
    pub ctl_ridx: usize,
    pub sp_ack_dur: u16,
    pub lp_ack_dur: u16,
}

#[rustfmt::skip]
pub const RATES: [Rate; 12] = [
    Rate { rate: 2, mcs: 0, ofdm: false, ctl_ridx: 0, sp_ack_dur: 314, lp_ack_dur: 314 },
    Rate { rate: 4, mcs: 1, ofdm: false, ctl_ridx: 1, sp_ack_dur: 258, lp_ack_dur: 162 },
    Rate { rate: 11, mcs: 2, ofdm: false, ctl_ridx: 2, sp_ack_dur: 223, lp_ack_dur: 127 },
    Rate { rate: 22, mcs: 3, ofdm: false, ctl_ridx: 3, sp_ack_dur: 213, lp_ack_dur: 117 },
    Rate { rate: 12, mcs: 0, ofdm: true, ctl_ridx: 4, sp_ack_dur: 60, lp_ack_dur: 60 },
    Rate { rate: 18, mcs: 1, ofdm: true, ctl_ridx: 4, sp_ack_dur: 52, lp_ack_dur: 52 },
    Rate { rate: 24, mcs: 2, ofdm: true, ctl_ridx: 6, sp_ack_dur: 48, lp_ack_dur: 48 },
    Rate { rate: 36, mcs: 3, ofdm: true, ctl_ridx: 6, sp_ack_dur: 44, lp_ack_dur: 44 },
    Rate { rate: 48, mcs: 4, ofdm: true, ctl_ridx: 8, sp_ack_dur: 44, lp_ack_dur: 44 },
    Rate { rate: 72, mcs: 5, ofdm: true, ctl_ridx: 8, sp_ack_dur: 40, lp_ack_dur: 40 },
    Rate { rate: 96, mcs: 6, ofdm: true, ctl_ridx: 8, sp_ack_dur: 40, lp_ack_dur: 40 },
    Rate { rate: 108, mcs: 7, ofdm: true, ctl_ridx: 8, sp_ack_dur: 40, lp_ack_dur: 40 },
];
pub const RIDX_CCK1: usize = 0;
pub const RIDX_OFDM24: usize = 6;

/// Each 2.4 GHz channel's RF bank 0 registers 17 to 20 (MT7601_RF_CHAN).
pub const RF_CHAN: [(u8, u8, u8, u8, u8); 14] = [
    (1, 0x99, 0x99, 0x09, 0x50), (2, 0x46, 0x44, 0x0a, 0x50),
    (3, 0xec, 0xee, 0x0a, 0x50), (4, 0x99, 0x99, 0x0b, 0x50),
    (5, 0x46, 0x44, 0x08, 0x51), (6, 0xec, 0xee, 0x08, 0x51),
    (7, 0x99, 0x99, 0x09, 0x51), (8, 0x46, 0x44, 0x0a, 0x51),
    (9, 0xec, 0xee, 0x0a, 0x51), (10, 0x99, 0x99, 0x0b, 0x51),
    (11, 0x46, 0x44, 0x08, 0x52), (12, 0xec, 0xee, 0x08, 0x52),
    (13, 0x99, 0x99, 0x09, 0x52), (14, 0x33, 0x33, 0x0b, 0x52),
];

/// The MAC's registers at init (MT7601_DEF_MAC).
pub const DEF_MAC: [(u16, u32); 67] = [
    (0x041c, 0x18100800), (0x0420, 0x38302820), (0x0424, 0x58504840),
    (0x0428, 0x78706860), (0x1004, 0x0000000c), (0x1018, 0x000a3fff),
    (0x1030, 0x77777777), (0x1034, 0x77777777), (0x1038, 0x77777777),
    (0x103c, 0x77777777), (0x1100, 0x33a41010), (0x1104, 0x00000209),
    (0x1118, 0x00422010), (0x1128, 0x00000000), (0x1204, 0x00000000),
    (0x1208, 0x00000014), (0x1300, 0x000a4360), (0x1304, 0x000a4700),
    (0x1308, 0x00043338), (0x130c, 0x0003222f), (0x1328, 0x33150f0f),
    (0x132c, 0x00000005), (0x1330, 0x00000402), (0x1334, 0x00000000),
    (0x1338, 0x00000000), (0x1340, 0x0000583f), (0x1344, 0x01100020),
    (0x1348, 0x000a2090), (0x134c, 0x47d01f0f), (0x1350, 0x007f1820),
    (0x1358, 0xedcba980), (0x1364, 0x07f40000), (0x1368, 0x07f60000),
    (0x136c, 0x01750003), (0x1370, 0x03f50003), (0x1374, 0x01750003),
    (0x1378, 0x03f50003), (0x1380, 0x002400ca), (0x1384, 0x00000000),
    (0x1388, 0x01010101), (0x13a0, 0x003b0005), (0x13a4, 0x00000000),
    (0x13a8, 0x00006969), (0x13ac, 0x6c6c6c6c), (0x13b0, 0x2f2f0005),
    (0x13c0, 0x00000400), (0x13c8, 0x00060006), (0x1400, 0x00015f97),
    (0x1404, 0x00000003), (0x1408, 0x0000015f), (0x140c, 0x00008003),
    (0x040c, 0x0000009f), (0x0208, 0x00000030), (0x0214, 0x00002273),
    (0x0218, 0x00002344), (0x021c, 0x000034aa), (0x0250, 0x00000000),
    (0x0400, 0x00080c00), (0x0800, 0x00000001), (0x120c, 0x00000000),
    (0x1214, 0x010055ff), (0x1218, 0x00550055), (0x121c, 0x010055ff),
    (0x1220, 0x00550055), (0x0a38, 0x00000000), (0x101c, 0x00000000),
    (0x0404, 0x7f723c1f),
];

/// The baseband's registers at init (MT7601_DEF_BBP).
pub const DEF_BBP: [(u8, u8); 150] = [
    (1, 0x04), (4, 0x40), (20, 0x06), (31, 0x08), (178, 0xff), (66, 0x14),
    (68, 0x8b), (69, 0x12), (70, 0x09), (73, 0x11), (75, 0x60), (76, 0x44),
    (84, 0x9a), (86, 0x38), (91, 0x07), (92, 0x02), (99, 0x50), (101, 0x00),
    (103, 0xc0), (104, 0x92), (105, 0x3c), (106, 0x03), (128, 0x12), (142, 0x04),
    (143, 0x37), (142, 0x03), (143, 0x99), (160, 0xeb), (161, 0xc4), (162, 0x77),
    (163, 0xf9), (164, 0x88), (165, 0x80), (166, 0xff), (167, 0xe4), (195, 0x00),
    (196, 0x00), (195, 0x01), (196, 0x04), (195, 0x02), (196, 0x20), (195, 0x03),
    (196, 0x0a), (195, 0x06), (196, 0x16), (195, 0x07), (196, 0x05), (195, 0x08),
    (196, 0x37), (195, 0x0a), (196, 0x15), (195, 0x0b), (196, 0x17), (195, 0x0c),
    (196, 0x06), (195, 0x0d), (196, 0x09), (195, 0x0e), (196, 0x05), (195, 0x0f),
    (196, 0x09), (195, 0x10), (196, 0x20), (195, 0x20), (196, 0x17), (195, 0x21),
    (196, 0x06), (195, 0x22), (196, 0x09), (195, 0x23), (196, 0x17), (195, 0x24),
    (196, 0x06), (195, 0x25), (196, 0x09), (195, 0x26), (196, 0x17), (195, 0x27),
    (196, 0x06), (195, 0x28), (196, 0x09), (195, 0x29), (196, 0x05), (195, 0x2a),
    (196, 0x09), (195, 0x80), (196, 0x8b), (195, 0x81), (196, 0x12), (195, 0x82),
    (196, 0x09), (195, 0x83), (196, 0x17), (195, 0x84), (196, 0x11), (195, 0x85),
    (196, 0x00), (195, 0x86), (196, 0x00), (195, 0x87), (196, 0x18), (195, 0x88),
    (196, 0x60), (195, 0x89), (196, 0x44), (195, 0x8a), (196, 0x8b), (195, 0x8b),
    (196, 0x8b), (195, 0x8c), (196, 0x8b), (195, 0x8d), (196, 0x8b), (195, 0x8e),
    (196, 0x09), (195, 0x8f), (196, 0x09), (195, 0x90), (196, 0x09), (195, 0x91),
    (196, 0x09), (195, 0x92), (196, 0x11), (195, 0x93), (196, 0x11), (195, 0x94),
    (196, 0x11), (195, 0x95), (196, 0x11), (47, 0x80), (60, 0x80), (150, 0xd2),
    (151, 0x32), (152, 0x23), (153, 0x41), (154, 0x00), (155, 0x4f), (253, 0x7e),
    (195, 0x30), (196, 0x32), (195, 0x31), (196, 0x23), (195, 0x32), (196, 0x45),
    (195, 0x35), (196, 0x4a), (195, 0x36), (196, 0x5a), (195, 0x37), (196, 0x5a),
];

/// The RF banks 0, 4 and 5 at init (MT7601_BANK0_RF, ...).
pub const RF_BANK0: [(u8, u8); 44] = [
    (0, 0x02), (1, 0x01), (2, 0x11), (3, 0xff), (4, 0x0a), (5, 0x20),
    (6, 0x00), (7, 0x00), (8, 0x00), (9, 0x00), (10, 0x00), (11, 0x21),
    (13, 0x00), (14, 0x7c), (15, 0x22), (16, 0x80), (17, 0x99), (18, 0x99),
    (19, 0x09), (20, 0x50), (21, 0xb0), (22, 0x00), (23, 0xc5), (24, 0xfc),
    (25, 0x40), (26, 0x4d), (27, 0x02), (28, 0x72), (29, 0x01), (30, 0x00),
    (31, 0x00), (32, 0x00), (33, 0x00), (34, 0x23), (35, 0x01), (36, 0x00),
    (37, 0x00), (38, 0x00), (39, 0x20), (40, 0x00), (41, 0xd0), (42, 0x1b),
    (43, 0x02), (44, 0x00),
];
pub const RF_BANK4: [(u8, u8); 60] = [
    (0, 0x01), (1, 0x00), (2, 0x00), (3, 0x00), (4, 0x00), (5, 0x08),
    (6, 0x00), (7, 0x5b), (8, 0x52), (9, 0xb6), (10, 0x57), (11, 0x33),
    (12, 0x22), (13, 0x3d), (14, 0x3e), (15, 0x13), (16, 0x22), (17, 0x23),
    (18, 0x02), (19, 0xa4), (20, 0x01), (21, 0x12), (22, 0x80), (23, 0xb3),
    (24, 0x00), (25, 0x00), (26, 0x00), (27, 0x00), (28, 0x18), (29, 0xee),
    (30, 0x6b), (31, 0x31), (32, 0x5d), (33, 0x00), (34, 0x96), (35, 0x55),
    (36, 0x08), (37, 0xbb), (38, 0xb3), (39, 0xb3), (40, 0x03), (41, 0x00),
    (42, 0x00), (43, 0xc5), (44, 0xc5), (45, 0xc5), (46, 0x07), (47, 0xa8),
    (48, 0xef), (49, 0x1a), (54, 0x07), (55, 0xa7), (56, 0xcc), (57, 0x14),
    (58, 0x07), (59, 0xa8), (60, 0xd7), (61, 0x10), (62, 0x1c), (63, 0x00),
];
pub const RF_BANK5: [(u8, u8); 59] = [
    (0, 0x47), (1, 0x00), (2, 0x00), (3, 0x08), (4, 0x04), (5, 0x20),
    (6, 0x3a), (7, 0x3a), (8, 0x00), (9, 0x00), (10, 0x10), (11, 0x10),
    (12, 0x10), (13, 0x10), (14, 0x10), (15, 0x20), (16, 0x22), (17, 0x7c),
    (18, 0x00), (19, 0x00), (20, 0x00), (21, 0xf1), (22, 0x11), (23, 0x02),
    (24, 0x41), (25, 0x20), (26, 0x00), (27, 0xd7), (28, 0xa2), (29, 0x20),
    (30, 0x49), (31, 0x20), (32, 0x04), (33, 0xf1), (34, 0xa1), (35, 0x01),
    (41, 0x00), (42, 0x00), (43, 0x00), (44, 0x00), (45, 0x00), (46, 0x00),
    (47, 0x00), (48, 0x00), (49, 0x00), (50, 0x00), (51, 0x00), (52, 0x00),
    (53, 0x00), (54, 0x00), (55, 0x00), (56, 0x00), (57, 0x00), (58, 0x31),
    (59, 0x31), (60, 0x0a), (61, 0x02), (62, 0x00), (63, 0x00),
];
