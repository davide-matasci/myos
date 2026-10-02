//! ELF constants for this arch's dynamic relocations.

pub const EXPECT_MACHINE: u16 = 183; // EM_AARCH64
pub const R_ABS64: u32 = 257; // R_AARCH64_ABS64
pub const R_GLOB_DAT: u32 = 1025;
pub const R_JUMP_SLOT: u32 = 1026;
pub const R_RELATIVE: u32 = 1027;
