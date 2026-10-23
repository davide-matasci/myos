//! The radios the driver takes: what tells them apart, as OpenBSD's
//! mtw_attach and mtw_load_microcode set them up (notices in LICENSE).
//! They share the MAC, its registers and descriptors; each has its ASIC,
//! its firmware file and where the firmware goes. Only the MT7601U is
//! here: the MT7610U and MT7612U (5 GHz, other RF) would add their own
//! radio setup to `dev` too (issue #553).

/// One radio.
#[derive(Debug, PartialEq, Eq)]
pub struct Chip {
    pub name: &'static str,
    /// Its USB ids {vendor, product}: adapters of many brands.
    pub ids: &'static [(u16, u16)],
    /// ASIC_VER's high half: the chip it must be.
    pub asic_ver: u16,
    /// Its firmware in linux-firmware (`wifi-firmware fetch`).
    pub firmware: &'static str,
    /// Where the firmware's instruction (ILM) and data (DLM) parts go in
    /// the MCU's memory (no DLM written: 0).
    pub ilm_offset: u32,
    pub dlm_offset: u32,
    /// The body of the HT Capabilities element the station offers: what
    /// it receives over 802.11n (the driver sends no aggregates and takes
    /// none: it declines block ack).
    pub ht_cap: [u8; 26],
}

/// One stream at 20 MHz: SM power save off, short guard interval, one
/// space-time block coding stream received; MCS 0 to 7, the TX MCS set
/// the RX one's.
pub const HT_CAP_1X1: [u8; 26] = [
    0x2c, 0x01, // HT capabilities info
    0x00, // A-MPDU parameters
    0xff, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0x01, 0, 0, 0, // supported MCS set
    0, 0, // HT extended capabilities
    0, 0, 0, 0, // transmit beamforming
    0, // ASEL
];

/// The MT7601U: 802.11b/g/n, 2.4 GHz, one antenna, high-speed USB. Cheap
/// "150 Mbps" dongles of every brand, a usual choice for a Raspberry Pi
/// or a RISC-V board with a USB port (OpenBSD's mtw_devs).
pub const MT7601U: Chip = Chip {
    name: "MT7601U",
    ids: &[
        (0x0b05, 0x17d3), // ASUS USB-N10 v2
        (0x13d3, 0x3431), // AzureWave
        (0x13d3, 0x3434),
        (0x2001, 0x3d04), // D-Link DWA-127 rev B1
        (0x7392, 0x7710), // Edimax EW-7711UAn V2
        (0x0e8d, 0x760a), // MediaTek
        (0x0e8d, 0x760b),
        (0x148f, 0x7601), // Ralink
        (0x148f, 0x760a),
        (0x148f, 0x760b),
        (0x148f, 0x760c),
        (0x148f, 0x760d),
        (0x2717, 0x4106), // Xiaomi
    ],
    asic_ver: 0x7601,
    firmware: "mt7601u.bin",
    ilm_offset: 0x40,
    dlm_offset: 0,
    ht_cap: HT_CAP_1X1,
};

pub const CHIPS: &[&Chip] = &[&MT7601U];

impl Chip {
    /// The radio USB id `id` is.
    pub fn by_id(id: (u16, u16)) -> Option<&'static Chip> {
        CHIPS.iter().copied().find(|c| c.ids.contains(&id))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn by_id() {
        assert_eq!(Chip::by_id((0x148f, 0x7601)), Some(&MT7601U));
        assert_eq!(Chip::by_id((0x2717, 0x4106)), Some(&MT7601U));
        assert_eq!(Chip::by_id((0x148f, 0x761a)), None);
        // No id is two radios'.
        let ids: alloc::vec::Vec<_> = CHIPS.iter().flat_map(|c| c.ids).collect();
        assert!(ids.iter().enumerate().all(|(i, id)| !ids[i + 1..].contains(id)));
    }
}
