//! The radios the driver takes: what tells them apart, as OpenBSD's
//! iwx_attach sets it up per PCI product (notices in LICENSE). They share
//! the 22000 family's rings, commands and firmware protocol; each has its
//! own MAC, firmware file and platform facts.

/// One radio.
#[derive(Debug, PartialEq, Eq)]
pub struct Chip {
    pub name: &'static str,
    /// Its PCI ids {vendor, device}.
    pub ids: &'static [(u16, u16)],
    /// HW_REV in the driver's form (steps in bits 2-3), under `hw_rev_mask`:
    /// the MAC it must have.
    pub hw_rev: u32,
    pub hw_rev_mask: u32,
    /// Integrated (CNVi: the MAC in Intel's chipset, the radio on a module)
    /// or discrete (a PCIe card): how it is started and told its latency.
    pub integrated: bool,
    /// The SoC's crystal latency the firmware is told (SOC_CONFIGURATION).
    pub xtal_latency_us: u32,
    /// Its firmware in linux-firmware (`wifi-firmware fetch`).
    pub firmware: &'static str,
}

/// The AX201: a CNVi module whose MAC is in the chipset, QuZ (Comet Lake H,
/// the ThinkPad T15p's, and Comet Lake LP). Intel x86 machines only.
pub const AX201: Chip = Chip {
    name: "AX201",
    ids: &[(0x8086, 0x06f0), (0x8086, 0x02f0)],
    hw_rev: 0x354,
    hw_rev_mask: 0xffff,
    integrated: true,
    xtal_latency_us: 500,
    firmware: "iwlwifi-QuZ-a0-hr-b0-77.ucode",
};

/// The AX200: the same generation on an M.2 PCIe card, its own CC MAC (any
/// step), which any machine with a slot for it can take (on aarch64 and
/// riscv64 its interrupt is the INTx line).
pub const AX200: Chip = Chip {
    name: "AX200",
    ids: &[(0x8086, 0x2723)],
    hw_rev: 0x340,
    hw_rev_mask: 0xfff0,
    integrated: false,
    xtal_latency_us: 0,
    firmware: "iwlwifi-cc-a0-77.ucode",
};

pub const CHIPS: &[&Chip] = &[&AX201, &AX200];

impl Chip {
    /// The radio PCI id `id` is.
    pub fn by_id(id: (u16, u16)) -> Option<&'static Chip> {
        CHIPS.iter().copied().find(|c| c.ids.contains(&id))
    }

    /// Whether HW_REV (in the driver's form) is this radio's MAC.
    pub fn has_mac(&self, hw_rev: u32) -> bool {
        hw_rev & self.hw_rev_mask == self.hw_rev
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn by_id_and_mac() {
        assert_eq!(Chip::by_id((0x8086, 0x06f0)), Some(&AX201));
        assert_eq!(Chip::by_id((0x8086, 0x2723)), Some(&AX200));
        assert_eq!(Chip::by_id((0x8086, 0x2725)), None);
        // The AX201 is a QuZ, step B; any CC step is an AX200's.
        assert!(AX201.has_mac(0x354) && !AX201.has_mac(0x350) && !AX201.has_mac(0x344));
        assert!(AX200.has_mac(0x340) && AX200.has_mac(0x344) && !AX200.has_mac(0x354));
        // No id is two radios'.
        let ids: alloc::vec::Vec<_> = CHIPS.iter().flat_map(|c| c.ids).collect();
        assert!(ids.iter().enumerate().all(|(i, id)| !ids[i + 1..].contains(id)));
    }
}
