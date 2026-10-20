//! No IOMMU driver on this arch yet: every device reaches all of memory,
//! `/dev/pci` is unconfined (`/proc/iommu` says `none`).

use alloc::string::String;

pub fn init() {}

pub fn attach(_bus: u8, _slot: u8, _func: u8) -> Option<u32> {
    None
}

pub fn map(_domain: u32, _phys: u64, _len: usize) -> bool {
    false
}

pub fn detach(_bus: u8, _slot: u8, _func: u8) {}

pub fn text() -> String {
    String::from("none\n")
}
