//! The device's registers (BAR0 offsets) and internal ("periphery")
//! addresses, from OpenBSD's if_iwxreg.h (notices in LICENSE). Only what the
//! 22000 family (AX200/AX201) uses.

// Control and status registers.
pub const HW_IF_CONFIG: u32 = 0x000;
pub const INT_COALESCING: u32 = 0x004;
pub const INT_PERIODIC: u32 = 0x005;
pub const INT: u32 = 0x008;
pub const INT_MASK: u32 = 0x00c;
pub const FH_INT_STATUS: u32 = 0x010;
pub const RESET: u32 = 0x020;
pub const GP_CNTRL: u32 = 0x024;
pub const HW_REV: u32 = 0x028;
pub const GIO: u32 = 0x03c;
pub const CTXT_INFO_BA: u32 = 0x040;
pub const UCODE_DRV_GP1_CLR: u32 = 0x05c;
pub const MBOX_SET: u32 = 0x088;
pub const HW_RF_ID: u32 = 0x09c;
pub const DRAM_INT_TBL: u32 = 0x0a0;
pub const MAC_SHADOW_CTRL: u32 = 0x0a8;
pub const GIO_CHICKEN: u32 = 0x100;
pub const DBG_HPET_MEM: u32 = 0x240;
pub const DBG_LINK_PWR_MGMT: u32 = 0x250;
/// Where the MAC address is (OTP at +0/+4, strap at +8/+0xc).
pub const MAC_ADDR: u32 = 0x380;

// HW_IF_CONFIG.
pub const HW_IF_MAC_DASH_POS: u32 = 0;
pub const HW_IF_MAC_STEP_POS: u32 = 2;
pub const HW_IF_PHY_TYPE_POS: u32 = 10;
pub const HW_IF_PHY_DASH_POS: u32 = 12;
pub const HW_IF_PHY_STEP_POS: u32 = 14;
pub const HW_IF_MAC_DASH: u32 = 0x3;
pub const HW_IF_MAC_STEP: u32 = 0xc;
pub const HW_IF_MAC_SI: u32 = 0x100;
pub const HW_IF_RADIO_SI: u32 = 0x200;
pub const HW_IF_PHY_TYPE: u32 = 0xc00;
pub const HW_IF_PHY_DASH: u32 = 0x3000;
pub const HW_IF_PHY_STEP: u32 = 0xc000;
pub const HW_IF_HAP_WAKE_L1A: u32 = 0x0008_0000;
pub const HW_IF_NIC_READY: u32 = 0x0040_0000;
pub const HW_IF_PREPARE: u32 = 0x0800_0000;

// INT (MSI) causes.
pub const INT_FH_RX: u32 = 1 << 31;
pub const INT_HW_ERR: u32 = 1 << 29;
pub const INT_RX_PERIODIC: u32 = 1 << 28;
pub const INT_FH_TX: u32 = 1 << 27;
pub const INT_SW_ERR: u32 = 1 << 25;
pub const INT_RF_KILL: u32 = 1 << 7;
pub const INT_CT_KILL: u32 = 1 << 6;
pub const INT_SW_RX: u32 = 1 << 3;
pub const INT_WAKEUP: u32 = 1 << 1;
pub const INT_ALIVE: u32 = 1 << 0;
pub const INT_INI_SET: u32 = INT_FH_RX
    | INT_HW_ERR
    | INT_FH_TX
    | INT_SW_ERR
    | INT_RF_KILL
    | INT_SW_RX
    | INT_WAKEUP
    | INT_ALIVE
    | INT_RX_PERIODIC;
pub const FH_INT_RX: u32 = (1 << 30) | (1 << 17) | (1 << 16);
pub const INT_PERIODIC_DIS: u8 = 0x00;
pub const INT_PERIODIC_ENA: u8 = 0xff;

// RESET, GP_CNTRL, the rest.
pub const RESET_SW: u32 = 0x80;
pub const RESET_LINK_PWR_MGMT_DISABLED: u32 = 0x8000_0000;
pub const GP_MAC_CLOCK_READY: u32 = 0x1;
pub const GP_INIT_DONE: u32 = 0x4;
pub const GP_MAC_ACCESS_REQ: u32 = 0x8;
pub const GP_GOING_TO_SLEEP: u32 = 0x10;
pub const GP_RFKILL_WAKE_L1A_EN: u32 = 0x0400_0000;
pub const GP_HW_RF_KILL_SW: u32 = 0x0800_0000;
pub const GIO_L0S_DISABLED: u32 = 0x2;
pub const GIO_CHICKEN_L1A_NO_L0S_RX: u32 = 0x0080_0000;
pub const DBG_HPET_MEM_VAL: u32 = 0xffff_0000;
pub const MBOX_OS_ALIVE: u32 = 0x20;
pub const UCODE_SW_BIT_RFKILL: u32 = 0x2;
pub const UCODE_DRV_GP1_CMD_BLOCKED: u32 = 0x4;
pub const DRAM_INT_TBL_ENABLE: u32 = 1 << 31;
pub const DRAM_INT_TBL_WRITE_POINTER: u32 = 1 << 28;
pub const DRAM_INT_TBL_WRAP_CHECK: u32 = 1 << 27;
pub const MAC_SHADOW_CTRL_VAL: u32 = 0x800f_ffff;

/// A discrete radio's latency tolerance (an integrated one's is in the
/// periphery, HPM_UMAC_LTR).
pub const LTR_LONG_VAL_AD: u32 = 0x0d4;

// The host bus window onto the device's memory and periphery.
pub const HBUS_TARG_MEM_RADDR: u32 = 0x40c;
pub const HBUS_TARG_MEM_RDAT: u32 = 0x41c;
pub const HBUS_TARG_PRPH_WADDR: u32 = 0x444;
pub const HBUS_TARG_PRPH_RADDR: u32 = 0x448;
pub const HBUS_TARG_PRPH_WDAT: u32 = 0x44c;
pub const HBUS_TARG_PRPH_RDAT: u32 = 0x450;
/// A transmit queue's write pointer: the queue << 16 | the index.
pub const HBUS_TARG_WRPTR: u32 = 0x460;

/// The receive queue 0's free buffer write index.
pub const RFH_Q0_FRBDCB_WIDX_TRG: u32 = 0x1c80;

// MSI-X.
pub const MSIX_FH_INT_CAUSES: u32 = 0x2800;
pub const MSIX_FH_INT_MASK: u32 = 0x2804;
pub const MSIX_HW_INT_CAUSES: u32 = 0x2808;
pub const MSIX_HW_INT_MASK: u32 = 0x280c;
pub const MSIX_AUTOMASK_ST: u32 = 0x2810;
pub const MSIX_RX_IVAR: u32 = 0x2880;
pub const MSIX_IVAR: u32 = 0x2890;
pub const MSIX_NON_AUTO_CLEAR: u8 = 1 << 7;
pub const MSIX_FH_Q0: u32 = 1 << 0;
pub const MSIX_FH_Q1: u32 = 1 << 1;
pub const MSIX_FH_D2S_CH0: u32 = 1 << 16;
pub const MSIX_FH_D2S_CH1: u32 = 1 << 17;
pub const MSIX_FH_S2D: u32 = 1 << 19;
pub const MSIX_FH_ERR: u32 = 1 << 21;
pub const MSIX_HW_ALIVE: u32 = 1 << 0;
pub const MSIX_HW_WAKEUP: u32 = 1 << 1;
pub const MSIX_HW_RESET_DONE: u32 = 1 << 2;
pub const MSIX_HW_TOP_FATAL_ERR: u32 = 1 << 3;
pub const MSIX_HW_SW_ERR_V2: u32 = 1 << 5;
pub const MSIX_HW_CT_KILL: u32 = 1 << 6;
pub const MSIX_HW_RF_KILL: u32 = 1 << 7;
pub const MSIX_HW_PERIODIC: u32 = 1 << 8;
pub const MSIX_HW_SW_ERR: u32 = 1 << 25;
pub const MSIX_HW_SCD: u32 = 1 << 26;
pub const MSIX_HW_FH_TX: u32 = 1 << 27;
pub const MSIX_HW_HW_ERR: u32 = 1 << 29;
pub const MSIX_HW_HAP: u32 = 1 << 30;
/// The causes routed to the one vector (enum msix_ivar_for_cause).
pub const MSIX_IVAR_CAUSES: [u32; 16] =
    [0x0, 0x1, 0x3, 0x5, 0x10, 0x11, 0x12, 0x16, 0x17, 0x18, 0x29, 0x15, 0x2a, 0x2b, 0x2d, 0x2e];

// Periphery addresses (through HBUS_TARG_PRPH_*).
pub const PRPH_MASK: u32 = 0x000f_ffff;
pub const HPM_UMAC_LTR: u32 = 0xa0_3480;
pub const HPM_MAC_LTR_CSR: u32 = 0xa0_348c;
pub const HPM_MAC_LRT_ENABLE_ALL: u32 = 0xf;
pub const HPM_DEBUG: u32 = 0xa0_3440;
pub const PERSISTENCE_BIT: u32 = 1 << 12;
pub const HPM_HIPM_GEN_CFG: u32 = 0xa0_3458;
pub const HIPM_PG_EN: u32 = 1 << 0;
pub const HIPM_SLP_EN: u32 = 1 << 1;
pub const HIPM_FORCE_ACTIVE: u32 = 1 << 10;
pub const PREG_PRPH_WPROT_22000: u32 = 0xa0_4d00;
pub const PREG_WFPM_ACCESS: u32 = 1 << 12;
pub const UREG_CHICK: u32 = 0xa0_5c00;
pub const UREG_CHICK_MSI_ENABLE: u32 = 1 << 24;
pub const UREG_CHICK_MSIX_ENABLE: u32 = 1 << 25;
pub const UREG_CPU_INIT_RUN: u32 = 0xa0_5c44;
