//! ELF constants for this arch's dynamic relocations.

pub const EXPECT_MACHINE: u16 = 243; // EM_RISCV
pub const R_ABS64: u32 = 2; // R_RISCV_64
pub const R_GLOB_DAT: u32 = 6;
pub const R_JUMP_SLOT: u32 = 5;
pub const R_RELATIVE: u32 = 3;
