//! ELF constants for this arch's dynamic relocations.

pub const EXPECT_MACHINE: u16 = 62; // EM_X86_64
pub const R_ABS64: u32 = 1; // R_X86_64_64
pub const R_GLOB_DAT: u32 = 6;
pub const R_JUMP_SLOT: u32 = 7;
pub const R_RELATIVE: u32 = 8;
