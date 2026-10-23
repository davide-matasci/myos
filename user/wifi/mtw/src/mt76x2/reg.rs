//! The MT76x2U's registers, vendor requests, ROM layout and MCU messages
//! (Linux's mt76x02_regs.h, mt76x02_eeprom.h, mt76x02_mcu.h, mt76x2_mcu.h,
//! mt76x02_dma.h, mt76x02_mac.h and mt76.h; notices in LICENSE). The
//! values are the macros' as the C preprocessor works them out.

// ---- Vendor requests (mt76.h, enum mt_vendor_req) -------------------------

/// The device's mode: 1 resets the MCU, 0x12 starts it on its vectors.
pub const REQ_DEV_MODE: u8 = 0x1;
pub const REQ_MULTI_WRITE: u8 = 0x6;
pub const REQ_MULTI_READ: u8 = 0x7;
pub const REQ_READ_EEPROM: u8 = 0x9;
/// A register of the FCE, written a half at a time (mt76u_single_wr).
pub const REQ_WRITE_FCE: u8 = 0x42;
pub const REQ_WRITE_CFG: u8 = 0x46;
pub const REQ_READ_CFG: u8 = 0x47;

/// DEV_MODE's values.
pub const DEV_MODE_RESET: u16 = 0x1;
pub const DEV_MODE_IVB: u16 = 0x12;

/// The address space a register is in (MT_VEND_TYPE_*): the MAC's (the
/// multi requests), the USB configuration's, the EEPROM's.
pub const TYPE_CFG: u32 = 1 << 30;
pub const TYPE_EEPROM: u32 = 1 << 31;
pub const TYPE_MASK: u32 = TYPE_CFG | TYPE_EEPROM;

// ---- The USB configuration space (MT_VEND_ADDR(CFG, …)) -----------------

pub const CFG_U3DMA: u32 = TYPE_CFG | 0x9018;
pub const DMA_RX_DROP_OR_PAD: u32 = 1 << 18;
pub const DMA_RX_BULK_AGG_EN: u32 = 1 << 21;
pub const DMA_RX_BULK_EN: u32 = 1 << 22;
pub const DMA_TX_BULK_EN: u32 = 1 << 23;
pub const DMA_RX_BUSY: u32 = 1 << 30;
pub const DMA_TX_BUSY: u32 = 1 << 31;
/// The bulk RX aggregation timeout field (its low byte).
pub const DMA_RX_BULK_AGG_TOUT: u32 = 0xff;

/// The WLAN power domain (MT_WLAN_MTC_CTRL, at 0x148), and the RF and
/// baseband power the bring-up turns on (mt76x2u_power_on and _rf).
pub const CFG_MTC_CTRL: u32 = TYPE_CFG | 0x148;
pub const MTC_MTCMOS_PWR_UP: u32 = 1 << 0;
pub const MTC_PWR_ACK: u32 = 1 << 12;
pub const MTC_PWR_ACK_S: u32 = 1 << 13;
pub const MTC_STATE_UP: u32 = 1 << 28;
pub const CFG_AD_DA: u32 = TYPE_CFG | 0x1204;
pub const CFG_WLAN_FUN: u32 = TYPE_CFG | 0x80;
pub const CFG_BBP_RESET: u32 = TYPE_CFG | 0x64;
pub const CFG_RF_POWER: u32 = TYPE_CFG | 0x130;
pub const CFG_RF_PATCH_1C: u32 = TYPE_CFG | 0x1c;
pub const CFG_RF_PATCH_14: u32 = TYPE_CFG | 0x14;
pub const CFG_RF_PATCH_14C: u32 = TYPE_CFG | 0x14c;

/// The crystal's trim (mt76x2u_mac_fixup_xtal).
pub const CFG_XO_CTRL5: u32 = TYPE_CFG | 0x114;
pub const XO_CTRL5_C2_VAL: u32 = 0x7f00;
pub const CFG_XO_CTRL6: u32 = TYPE_CFG | 0x118;
pub const XO_CTRL6_C2_CTRL: u32 = 0x7f00;
pub const XO_CTRL7: u32 = 0x11c;

// ---- The MAC's registers ------------------------------------------------

pub const ASIC_VERSION: u32 = 0x0000;
/// The chip's revision (ASIC_VERSION's low half) from which the ROM patch
/// is flagged in the MCU clock register, the DLM moves and the TXWI's
/// streams field changes (MT76XX_REV_E3, _E4).
pub const REV_E3: u16 = 0x22;
pub const REV_E4: u16 = 0x33;

pub const COEXCFG0: u32 = 0x0040;
pub const COEX_EN: u32 = 1 << 0;
pub const WLAN_FUN_CTRL: u32 = 0x0080;
pub const WLAN_EN: u32 = 1 << 0;
pub const WLAN_CLK_EN: u32 = 1 << 1;
pub const WLAN_RESET_RF: u32 = 1 << 2;
pub const FRC_WL_ANT_SEL: u32 = 1 << 5;

pub const WPDMA_GLO_CFG: u32 = 0x0208;
pub const TX_DMA_BUSY: u32 = 1 << 1;
pub const RX_DMA_BUSY: u32 = 1 << 3;
pub const WPDMA_DELAY_INT_CFG: u32 = 0x0210;
pub const WMM_AIFSN: u32 = 0x0214;
pub const WMM_CWMIN: u32 = 0x0218;
pub const WMM_CWMAX: u32 = 0x021c;
pub const FCE_DMA_ADDR: u16 = 0x0230;
pub const FCE_DMA_LEN: u16 = 0x0234;
pub const TSO_CTRL: u32 = 0x0250;
pub const HEADER_TRANS_CTRL: u32 = 0x0260;
pub const US_CYC_CFG: u32 = 0x02a4;
pub const US_CYC_CNT: u32 = 0xff;

pub const PBF_SYS_CTRL: u32 = 0x0400;
pub const PBF_CFG: u32 = 0x0404;
pub const PBF_TX_MAX_PCNT: u32 = 0x0408;
pub const PBF_RX_MAX_PCNT: u32 = 0x040c;
/// The page counts of the TX and RX queues, polled empty when the MAC
/// stops (mt76x2u_mac_stop's 0x0438, 0x0430, 0x0a30, 0x0a34).
pub const PBF_TXQ_PCNT: u32 = 0x0438;
pub const PBF_RXQ_PCNT: u32 = 0x0430;
pub const PSE_PCNT0: u32 = 0x0a30;
pub const PSE_PCNT1: u32 = 0x0a34;

pub const MCU_CLOCK_CTL: u32 = 0x0708;
pub const MCU_COM_REG0: u32 = 0x0730;
pub const MCU_SEMAPHORE_03: u32 = 0x07bc;

pub const FCE_PSE_CTRL: u32 = 0x0800;
pub const FCE_L2_STUFF: u32 = 0x080c;
pub const L2_STUFF_WR_MPDU_LEN_EN: u32 = 1 << 4;
pub const FCE_WLAN_FLOW_CONTROL1: u32 = 0x0824;
pub const TX_CPU_FCE_BASE: u32 = 0x09a0;
pub const TX_CPU_FCE_MAX_COUNT: u32 = 0x09a4;
pub const TX_CPU_FCE_DESC_IDX: u32 = 0x09a8;
pub const FCE_PDMA: u32 = 0x09c4;
pub const PAUSE_ENABLE_CONTROL1: u32 = 0x0a38;
pub const FCE_SKIP_FS: u32 = 0x0a6c;
pub const EFUSE_CTRL: u32 = 0x0024;

pub const MAC_CSR0: u32 = 0x1000;
pub const MAC_SYS_CTRL: u32 = 0x1004;
pub const RESET_CSR: u32 = 1 << 0;
pub const RESET_BBP: u32 = 1 << 1;
pub const ENABLE_TX: u32 = 1 << 2;
pub const ENABLE_RX: u32 = 1 << 3;
pub const MAC_ADDR_DW0: u32 = 0x1008;
pub const MAC_ADDR_DW1: u32 = 0x100c;
pub const U2ME_MASK_SHIFT: u32 = 16;
pub const MAC_BSSID_DW0: u32 = 0x1010;
pub const MAC_BSSID_DW1: u32 = 0x1014;
/// 8 access points and 8 stations (MBSS_MODE 3), the local bit, 7 more
/// beacon slots.
pub const MBSS_MODE_SHIFT: u32 = 16;
pub const MBSS_LOCAL_BIT: u32 = 1 << 21;
pub const MBEACON_N_SHIFT: u32 = 18;
pub const MAX_LEN_CFG: u32 = 0x1018;
pub const AMPDU_MAX_LEN_20M1S: u32 = 0x1030;
pub const AMPDU_MAX_LEN_20M2S: u32 = 0x1034;
/// The AP-client BSSID of station interface 8 (its bss index & 7):
/// MT_MAC_APC_BSSID_L(0) and _H(0), the address in H's low half.
pub const APC_BSSID_L0: u32 = 0x1090;
pub const APC_BSSID_H0: u32 = 0x1094;

pub const XIFS_TIME_CFG: u32 = 0x1100;
pub const OFDM_SIFS_SHIFT: u32 = 8;
pub const BKOFF_SLOT_CFG: u32 = 0x1104;
pub const SLOTTIME: u32 = 0xff;
pub const CC_DELAY_SHIFT: u32 = 8;
pub const CH_TIME_CFG: u32 = 0x110c;
/// The channel timer on, TX, RX, NAV and EIFS as busy, CCA counted, the
/// timer cleared (mt76x02_mac_cc_reset).
pub const CH_TIME_CFG_CC: u32 = 0x1 | 0x2 | 0x4 | 0x8 | 0x10 | 0x40 | 0x100;
pub const CH_IDLE: u32 = 0x1130;
pub const CH_BUSY: u32 = 0x1134;
pub const BEACON_TIME_CFG: u32 = 0x1114;
pub const BEACON_TIMER_EN: u32 = 1 << 16;
pub const BEACON_SYNC_MODE: u32 = 3 << 17;
pub const BEACON_TBTT_EN: u32 = 1 << 19;
pub const BEACON_TX: u32 = 1 << 20;
pub const TBTT_SYNC_CFG: u32 = 0x1118;
pub const ED_CCA_TIMER: u32 = 0x1140;
pub const BCN_BYPASS_MASK: u32 = 0x108c;

pub const MAC_STATUS: u32 = 0x1200;
pub const STATUS_TX: u32 = 1 << 0;
pub const STATUS_RX: u32 = 1 << 1;
pub const PWR_PIN_CFG: u32 = 0x1204;
pub const AUX_CLK_CFG: u32 = 0x120c;
pub const BB_PA_MODE_CFG0: u32 = 0x1214;
pub const BB_PA_MODE_CFG1: u32 = 0x1218;
pub const RF_PA_MODE_CFG0: u32 = 0x121c;
pub const RF_PA_MODE_CFG1: u32 = 0x1220;
pub const RF_PA_MODE_ADJ0: u32 = 0x1228;
pub const RF_PA_MODE_ADJ1: u32 = 0x122c;
pub const DACCLK_EN_DLY_CFG: u32 = 0x1264;

pub const TX_PWR_CFG_0: u32 = 0x1314;
pub const TX_PWR_CFG_1: u32 = 0x1318;
pub const TX_PWR_CFG_2: u32 = 0x131c;
pub const TX_PWR_CFG_3: u32 = 0x1320;
pub const TX_PWR_CFG_4: u32 = 0x1324;
pub const TX_PIN_CFG: u32 = 0x1328;
/// Both chains' PA and LNA, the RF and TR switches (mt76x02_edcca_tx_enable).
pub const TX_PIN_ON: u32 = 0xf | 0xf00 | 1 << 16 | 1 << 18;
pub const TX_BAND_CFG: u32 = 0x132c;
pub const BAND_UPPER_40M: u32 = 1 << 0;
pub const BAND_5G: u32 = 1 << 1;
pub const BAND_2G: u32 = 1 << 2;
pub const TX_SW_CFG0: u32 = 0x1330;
pub const TX_SW_CFG1: u32 = 0x1334;
pub const TX_SW_CFG2: u32 = 0x1338;
pub const TX_SW_CFG3: u32 = 0x1478;
pub const TXOP_CTRL_CFG: u32 = 0x1340;
pub const TXOP_ED_CCA_EN: u32 = 1 << 20;
pub const TX_RTS_CFG: u32 = 0x1344;
pub const RTS_RETRY_LIMIT: u32 = 0xff;
pub const TX_TIMEOUT_CFG: u32 = 0x1348;
pub const ACKTO_SHIFT: u32 = 8;
pub const TX_RETRY_CFG: u32 = 0x134c;
pub const TX_LINK_CFG: u32 = 0x1350;
pub const TX_CFACK_EN: u32 = 1 << 12;
pub const VHT_HT_FBK_CFG1: u32 = 0x1358;
pub const CCK_PROT_CFG: u32 = 0x1364;
pub const OFDM_PROT_CFG: u32 = 0x1368;
pub const MM20_PROT_CFG: u32 = 0x136c;
pub const MM40_PROT_CFG: u32 = 0x1370;
pub const GF20_PROT_CFG: u32 = 0x1374;
pub const GF40_PROT_CFG: u32 = 0x1378;
pub const EXP_ACK_TIME: u32 = 0x1380;
pub const HT_FBK_TO_LEGACY: u32 = 0x1384;
pub const TX0_RF_GAIN_CORR: u32 = 0x13a0;
pub const TX1_RF_GAIN_CORR: u32 = 0x13a4;
pub const TX_ALC_CFG_2: u32 = 0x13a8;
pub const TX_ALC_CFG_3: u32 = 0x13ac;
pub const TX_ALC_CFG_0: u32 = 0x13b0;
pub const TX_ALC_CFG_1: u32 = 0x13b4;
pub const TX_ALC_CFG_4: u32 = 0x13c0;
pub const TX_ALC_VGA3: u32 = 0x13c8;
pub const TX_PWR_CFG_7: u32 = 0x13d4;
pub const TX_PWR_CFG_8: u32 = 0x13d8;
pub const TX_PWR_CFG_9: u32 = 0x13dc;
pub const TX_PROT_CFG6: u32 = 0x13e0;
pub const TX_PROT_CFG7: u32 = 0x13e4;
pub const TX_PROT_CFG8: u32 = 0x13e8;
pub const PIFS_TX_CFG: u32 = 0x13ec;

pub const RX_FILTR_CFG: u32 = 0x1400;
/// Frames the receive filter drops (MT_RX_FILTR_CFG_*): with a bad FCS
/// or PHY, unicast to others (PROMISC), control frames, PS-Polls.
pub const FILTR_CRC_ERR: u32 = 1 << 0;
pub const FILTR_PHY_ERR: u32 = 1 << 1;
pub const FILTR_PROMISC: u32 = 1 << 2;
pub const FILTR_OTHER_BSS: u32 = 1 << 3;
pub const FILTR_CONTROL: u32 = 1 << 8 | 1 << 9 | 1 << 10 | 1 << 11 | 1 << 14 | 1 << 16;
pub const FILTR_PSPOLL: u32 = 1 << 13;
pub const AUTO_RSP_CFG: u32 = 0x1404;
pub const AUTO_RSP_EN: u32 = 1 << 0;
pub const AUTO_RSP_PREAMB_SHORT: u32 = 1 << 4;
pub const LEGACY_BASIC_RATE: u32 = 0x1408;
pub const HT_BASIC_RATE: u32 = 0x140c;
pub const HT_CTRL_CFG: u32 = 0x1410;
pub const EXT_CCA_CFG: u32 = 0x141c;
/// The extension channels' CCA, all four fields and the mask: at 20 MHz
/// CCA0..3 are 0..3 and the mask the primary's (ext_cca_chan[0]).
pub const EXT_CCA_MASK: u32 = 0xfff;
pub const EXT_CCA_20: u32 = 0 | 1 << 2 | 2 << 4 | 3 << 6 | 1 << 8;
pub const PN_PAD_MODE: u32 = 0x150c;
pub const TXOP_HLDR_ET: u32 = 0x1608;
pub const TX40M_BLK_EN: u32 = 1 << 1;
pub const PROT_AUTO_TX_CFG: u32 = 0x1648;

/// The statistics counters, cleared when read: RX errors and false CCA,
/// TX failures (CNT0's low half), successes and retries (CNT1's halves).
pub const RX_STAT_0: u32 = 0x1700;
pub const RX_STAT_1: u32 = 0x1704;
pub const RX_STAT_2: u32 = 0x1708;
pub const TX_STA_0: u32 = 0x170c;
pub const TX_STA_1: u32 = 0x1710;
pub const TX_STA_2: u32 = 0x1714;
pub const TX_STAT_FIFO: u32 = 0x1718;
/// The aggregate counters 0 to 7, then 8 to 15 (MT_TX_AGG_CNT).
pub const TX_AGG_CNT_BASE0: u32 = 0x1720;
pub const TX_AGG_CNT_BASE1: u32 = 0x174c;

/// The station table: each entry's address (8 bytes, the first 128) and
/// attributes (its bss index: 4 bits, the high one apart).
pub const WCID_ADDR_BASE: u32 = 0x1800;
pub const WCID_ATTR_BASE: u32 = 0xa800;
pub const WCID_ATTR_BSS_IDX_SHIFT: u32 = 4;
pub const WCID_ATTR_BSS_IDX_EXT: u32 = 1 << 11;
/// The shared keys of bss indexes 0 to 7, and 8 to 15 (4 of 32 bytes
/// each), and their ciphers (4 bits a key, two bss indexes a register).
pub const SKEY_BASE_0: u32 = 0xac00;
pub const SKEY_BASE_1: u32 = 0xb400;
pub const SKEY_MODE_BASE_0: u32 = 0xb000;
pub const SKEY_MODE_BASE_1: u32 = 0xb3f0;

pub const fn wcid_addr(n: u8) -> u32 {
    WCID_ADDR_BASE + n as u32 * 8
}

pub const fn wcid_attr(n: u8) -> u32 {
    WCID_ATTR_BASE + n as u32 * 4
}

/// The baseband's registers (MT_BBP(type, n)).
pub const fn bbp(base: u32, n: u32) -> u32 {
    base + (n << 2)
}
pub const BBP_CORE: u32 = 0x2000;
pub const BBP_IBI: u32 = 0x2100;
pub const BBP_AGC: u32 = 0x2300;
pub const BBP_TXO: u32 = 0x2600;
pub const BBP_TXBE: u32 = 0x2700;
pub const BBP_RXO: u32 = 0x2900;
/// AGC registers' fields: the high LNA gain, the gain; AGC 0's
/// bandwidth and control channel; CORE 1's bandwidth; TXBE 0's control
/// channel.
pub const AGC_LNA_HIGH_GAIN: u32 = 0x3f_0000;
pub const AGC_GAIN: u32 = 0x7f00;
pub const AGC_R0_BW: u32 = 0x7000;
pub const AGC_R0_CTRL_CHAN: u32 = 0x300;
pub const CORE_R1_BW: u32 = 0x18;
pub const TXBE_R0_CTRL_CHAN: u32 = 0x3;

// ---- The EEPROM (512 bytes, MT_VEND_ADDR(EEPROM, …)) --------------------

pub const EEPROM_SIZE: usize = 512;
pub const EE_CHIP_ID: usize = 0x000;
pub const EE_MAC_ADDR: usize = 0x004;
pub const EE_PCI_ID: usize = 0x00a;
pub const EE_NIC_CONF_0: usize = 0x034;
pub const EE_NIC_CONF_1: usize = 0x036;
pub const EE_XTAL_TRIM_1: usize = 0x03a;
pub const EE_NIC_CONF_2: usize = 0x042;
pub const EE_LNA_GAIN: usize = 0x044;
pub const EE_RSSI_OFFSET_2G_0: usize = 0x046;
pub const EE_RSSI_OFFSET_2G_1: usize = 0x048;
pub const EE_RSSI_OFFSET_5G_0: usize = 0x04a;
pub const EE_RSSI_OFFSET_5G_1: usize = 0x04c;
pub const EE_TX_POWER_EXT_PA_5G: usize = 0x054;
pub const EE_TX_POWER_0_START_2G: usize = 0x056;
pub const EE_TX_POWER_1_START_2G: usize = 0x05c;
/// Six groups of 5 bytes per chain on 5 GHz.
pub const EE_TX_POWER_0_START_5G: usize = 0x062;
pub const EE_TX_POWER_1_START_5G: usize = 0x080;
pub const TX_POWER_GROUP_SIZE_5G: usize = 5;
pub const EE_XTAL_TRIM_2: usize = 0x09e;
pub const EE_TX_POWER_CCK: usize = 0x0a0;
pub const EE_TX_POWER_OFDM_2G_6M: usize = 0x0a2;
pub const EE_TX_POWER_OFDM_2G_24M: usize = 0x0a4;
pub const EE_TX_POWER_HT_MCS0: usize = 0x0a6;
pub const EE_TX_POWER_HT_MCS4: usize = 0x0a8;
pub const EE_TX_POWER_HT_MCS8: usize = 0x0aa;
pub const EE_TX_POWER_HT_MCS12: usize = 0x0ac;
pub const EE_TX_POWER_OFDM_5G_6M: usize = 0x0b2;
pub const EE_TX_POWER_OFDM_5G_24M: usize = 0x0b4;
pub const EE_TX_POWER_VHT_MCS8: usize = 0x0be;
pub const EE_RF_2G_TSSI_OFF_TXPOWER: usize = 0x0f6;
pub const EE_RF_2G_RX_HIGH_GAIN: usize = 0x0f8;
pub const EE_RF_5G_GRP0_1_RX_HIGH_GAIN: usize = 0x0fa;
pub const EE_RF_5G_GRP2_3_RX_HIGH_GAIN: usize = 0x0fc;
pub const EE_RF_5G_GRP4_5_RX_HIGH_GAIN: usize = 0x0fe;
pub const EE_BT_RCAL_RESULT: usize = 0x138;

pub const NIC_CONF_0_PA_INT_2G: u16 = 1 << 8;
pub const NIC_CONF_0_PA_INT_5G: u16 = 1 << 9;
pub const NIC_CONF_0_BOARD_TYPE_SHIFT: u16 = 12;
pub const BOARD_TYPE_2GHZ: u16 = 1;
pub const BOARD_TYPE_5GHZ: u16 = 2;
pub const NIC_CONF_1_TEMP_TX_ALC: u16 = 1 << 1;
pub const NIC_CONF_1_LNA_EXT_2G: u16 = 1 << 2;
pub const NIC_CONF_1_LNA_EXT_5G: u16 = 1 << 3;
pub const NIC_CONF_1_TX_ALC_EN: u16 = 1 << 13;
pub const NIC_CONF_2_XTAL_OPTION_SHIFT: u16 = 9;

// ---- The MCU (mt76x02_mcu.h, mt76x2_mcu.h, mt76x02_dma.h) ---------------

/// A message's TXINFO: its length, sequence number, command, port and
/// type (MT_MCU_MSG_*); a frame's: its length, 802.11, WIV, queue
/// (MT_TXD_INFO_*).
pub const MSG_SEQ_SHIFT: u32 = 16;
pub const MSG_CMD_SHIFT: u32 = 20;
pub const MSG_PORT_SHIFT: u32 = 27;
pub const MSG_TYPE_CMD: u32 = 1 << 30;
pub const TXD_80211: u32 = 1 << 19;
pub const TXD_WIV: u32 = 1 << 24;
pub const TXD_QSEL_SHIFT: u32 = 25;
pub const QSEL_EDCA: u32 = 2;
/// enum dma_msg_port.
pub const WLAN_PORT: u32 = 0;
pub const CPU_TX_PORT: u32 = 2;
/// The answer's RX FCE info: its sequence number and event type.
pub const RX_FCE_SEQ_SHIFT: u32 = 16;
pub const RX_FCE_EVT_SHIFT: u32 = 20;
pub const EVT_CMD_DONE: u32 = 0;
/// A response transfer at most (MCU_RESP_URB_SIZE).
pub const MCU_RESP_SIZE: usize = 1024;

/// enum mcu_cmd.
pub const CMD_FUN_SET_OP: u8 = 1;
pub const CMD_LOAD_CR: u8 = 2;
pub const CMD_INIT_GAIN_OP: u8 = 3;
pub const CMD_POWER_SAVING_OP: u8 = 20;
pub const CMD_SWITCH_CHANNEL_OP: u8 = 30;
pub const CMD_CALIBRATION_OP: u8 = 31;
/// FUN_SET's Q_SELECT; POWER_SAVING's modes; LOAD_CR's MT_RF_BBP_CR.
pub const Q_SELECT: u32 = 1;
pub const RADIO_OFF: u32 = 0x30;
pub const RADIO_ON: u32 = 0x31;
pub const RF_BBP_CR: u8 = 2;

/// enum mcu_calibration.
pub const CAL_R: u32 = 1;
pub const CAL_TEMP_SENSOR: u32 = 2;
pub const CAL_RXDCOC: u32 = 3;
pub const CAL_RC: u32 = 4;
pub const CAL_LC: u32 = 6;
pub const CAL_TX_LOFT: u32 = 7;
pub const CAL_TXIQ: u32 = 8;
pub const CAL_TSSI: u32 = 9;
pub const CAL_TSSI_COMP: u32 = 10;
pub const CAL_DPD: u32 = 11;
pub const CAL_RXIQC_FI: u32 = 12;
pub const CAL_TX_SHAPING: u32 = 15;

/// Where the ROM patch, the firmware's instructions (ILM) and data (DLM)
/// go, and the most a transfer of each carries (mt76x2_usb_mcu.c).
pub const ROM_PATCH_OFFSET: u32 = 0x9_0000;
pub const ILM_OFFSET: u32 = 0x8_0000;
pub const DLM_OFFSET: u32 = 0x11_0000;
pub const DLM_OFFSET_E3: u32 = 0x800;
pub const ROM_PATCH_MAX_PAYLOAD: usize = 2048;
pub const FW_MAX_PAYLOAD: usize = 0x3900;

/// The MCU's commands to apply the ROM patch, then to reset its WMT
/// (mt76x2u_mcu_enable_patch, _reset_wmt): class requests.
pub const PATCH_ENABLE: [u8; 11] = [0x6f, 0xfc, 0x08, 0x01, 0x20, 0x04, 0x00, 0x00, 0x00, 0x09, 0x00];
pub const PATCH_RESET_WMT: [u8; 8] = [0x6f, 0xfc, 0x05, 0x01, 0x07, 0x01, 0x00, 0x04];

// ---- Frames (mt76x02_mac.h, dma.h) --------------------------------------

/// A transfer's DMA header (its length), the RXWI, and the FCE info after.
pub const DMA_HDR_LEN: usize = 4;
pub const RXWI_LEN: usize = 32;
pub const FCE_INFO_LEN: usize = 4;
pub const TXWI_LEN: usize = 20;

/// RXWI rxinfo's bits.
pub const RXINFO_CRCERR: u32 = 1 << 8;
pub const RXINFO_ICVERR: u32 = 1 << 9;
pub const RXINFO_MICERR: u32 = 1 << 10;
pub const RXINFO_L2PAD: u32 = 1 << 14;
pub const RXINFO_DECRYPT: u32 = 1 << 16;
/// RXWI ctl's: the station entry, the MPDU's length.
pub const RXWI_CTL_WCID: u32 = 0xff;
pub const RXWI_CTL_MPDU_LEN_SHIFT: u32 = 16;
pub const RXWI_CTL_MPDU_LEN: u32 = 0x3fff;

/// A rate word (TXWI and RXWI): the MCS or legacy code, the bandwidth,
/// the PHY mode.
pub const RATE_PHY_SHIFT: u16 = 13;
pub const PHY_CCK: u16 = 0;
pub const PHY_OFDM: u16 = 1;
pub const PHY_HT: u16 = 2;
/// A CCK code's short preamble flag (hw_value_short: the code + 8).
pub const CCK_SHORT: u16 = 8;
/// TXWI ack_ctl: an ACK wanted, the sequence number the MAC's.
pub const TXWI_ACK_REQ: u8 = 1 << 0;
pub const TXWI_ACK_NSEQ: u8 = 1 << 1;
/// TXWI txstream: two streams on revision E4 and later, and on E3 for a
/// legacy rate.
pub const TXSTREAM_E4: u8 = 0x13;
pub const TXSTREAM_E3_LEGACY: u8 = 0x93;
