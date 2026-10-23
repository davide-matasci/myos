//! The radios the crate drives: what tells them apart. The MT7601U is
//! OpenBSD's mtw(4)'s (mtw_attach, mtw_load_microcode; `dev`), the
//! MT7612U Linux's mt76x2u's (mt76x2u_probe, mt76x2u_mcu_fw_init;
//! `mt76x2`); notices in LICENSE. Each has its ASICs, its firmware files
//! and where the firmware goes. The MT7610U, the other 802.11ac one, is
//! not here: Linux's driver for it is GPL-2.0-only, and OpenBSD's mtw does
//! not take it.

/// Which of the crate's drivers takes a radio.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum Family {
    /// `dev`: one stream, 2.4 GHz.
    Mt7601,
    /// `mt76x2`: two streams, 2.4 and 5 GHz.
    Mt76x2,
}

/// One radio.
#[derive(Debug, PartialEq, Eq)]
pub struct Chip {
    pub name: &'static str,
    pub family: Family,
    /// Its USB ids {vendor, product}: adapters of many brands.
    pub ids: &'static [(u16, u16)],
    /// ASIC_VER's high half: the chips it may be.
    pub asic: &'static [u16],
    /// Its firmware in linux-firmware (`wifi-firmware fetch`), and the ROM
    /// patch its MCU takes first, when it has one.
    pub firmware: &'static str,
    pub rom_patch: Option<&'static str>,
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

/// Two streams at 20 MHz: the same, LDPC received too; MCS 0 to 15.
pub const HT_CAP_2X2: [u8; 26] = [
    0x2d, 0x01, // HT capabilities info
    0x00, // A-MPDU parameters
    0xff, 0xff, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0x01, 0, 0, 0, // supported MCS set
    0, 0, // HT extended capabilities
    0, 0, 0, 0, // transmit beamforming
    0, // ASEL
];

/// The MT7601U: 802.11b/g/n, 2.4 GHz, one antenna, high-speed USB. Cheap
/// "150 Mbps" dongles of every brand, a usual choice for a Raspberry Pi
/// or a RISC-V board with a USB port (OpenBSD's mtw_devs).
pub const MT7601U: Chip = Chip {
    name: "MT7601U",
    family: Family::Mt7601,
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
    asic: &[0x7601],
    firmware: "mt7601u.bin",
    rom_patch: None,
    ilm_offset: 0x40,
    dlm_offset: 0,
    ht_cap: HT_CAP_1X1,
};

/// The MT7612U: 802.11a/b/g/n/ac, 2.4 and 5 GHz, two antennas, USB 3.
/// "AC1200" dongles (Linux's mt76x2u_device_table); the driver takes them
/// as 802.11n radios at 20 MHz (issue #563). It runs the MT7662E's
/// firmware, as Linux does.
pub const MT7612U: Chip = Chip {
    name: "MT7612U",
    family: Family::Mt76x2,
    ids: &[
        (0x0b05, 0x1833), // ASUS USB-AC54
        (0x0b05, 0x17eb), // ASUS USB-AC55
        (0x0b05, 0x180b), // ASUS USB-N53 B1
        (0x0e8d, 0x7612), // Aukey USBAC1200, Alfa AWUS036ACM
        (0x057c, 0x8503), // AVM FRITZ!WLAN AC860
        (0x7392, 0xb711), // Edimax EW-7722UAC
        (0x056e, 0x400a), // ELECOM WDC-867SU3S
        (0x0e8d, 0x7632), // HC-M7662BU1
        (0x0471, 0x2126), // LiteOn WN4516R module
        (0x0471, 0x7600), // LiteOn WN4519R module
        (0x2c4e, 0x0103), // Mercury UD13
        (0x0846, 0x9014), // Netgear WNDA3100v3
        (0x0846, 0x9053), // Netgear A6210
        (0x045e, 0x02e6), // Xbox One Wireless Adapter
        (0x045e, 0x02fe),
        (0x2357, 0x0137), // TP-Link TL-WDN6200
    ],
    asic: &[0x7612, 0x7632, 0x7662, 0x7602],
    firmware: "mt7662.bin",
    rom_patch: Some("mt7662_rom_patch.bin"),
    ilm_offset: crate::mt76x2::reg::ILM_OFFSET,
    dlm_offset: crate::mt76x2::reg::DLM_OFFSET,
    ht_cap: HT_CAP_2X2,
};

pub const CHIPS: &[&Chip] = &[&MT7601U, &MT7612U];

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
        assert_eq!(Chip::by_id((0x0846, 0x9053)), Some(&MT7612U));
        assert_eq!(Chip::by_id((0x148f, 0x761a)), None);
        // No id is two radios'.
        let ids: alloc::vec::Vec<_> = CHIPS.iter().flat_map(|c| c.ids).collect();
        assert!(ids.iter().enumerate().all(|(i, id)| !ids[i + 1..].contains(id)));
    }
}
