//! A firmware file for the simulated AX201 of the test boots (the
//! launcher's device model, `src/ax201_sim.rs`): shaped as Intel's
//! iwlwifi-QuZ-a0-hr-b0-77.ucode (its header, its records, the API and
//! capability bits, command versions and limits the driver looks at), but
//! its sections are myos's own bytes, which the model checks the driver
//! copied where the context information says. Nothing of Intel's is in it.

use alloc::vec::Vec;

/// The file's name, as the daemon looks for it.
pub const NAME: &str = "iwlwifi-QuZ-a0-hr-b0-77.ucode";

/// A section: where it loads in the device, and its length; its bytes are
/// [`byte`] of its index.
pub struct SimSection {
    pub devoff: u32,
    pub len: usize,
}

/// LMAC, UMAC and paging sections: one more than a page and one that
/// ends mid-page in each image, so the packing is exercised.
pub const LMAC: [SimSection; 2] = [SimSection { devoff: 0x0040_0000, len: 1000 }, SimSection { devoff: 0x0040_8000, len: 5000 }];
pub const UMAC: [SimSection; 2] = [SimSection { devoff: 0x0080_0000, len: 4096 }, SimSection { devoff: 0x0080_4000, len: 300 }];
pub const PAGING: [SimSection; 2] = [SimSection { devoff: 0x0090_0000, len: 4096 }, SimSection { devoff: 0x0090_1000, len: 100 }];

/// The LMAC's error log, where the model writes a firmware error's id.
pub const LMAC_ERROR_TABLE: u32 = 0x0040_2000;

/// The API and capability bits of the real file (iwx-map.md): MCC update,
/// the reduced scan configuration, extended scan channels; LAR, UMAC scan,
/// DS parameter set, binding/CDB, ultra high band channels, session
/// protection, CT kill by the firmware, MCC 11ax.
pub const API: &[usize] = &[9, 48, 56, 58];
pub const CAPA: &[usize] = &[1, 2, 9, 38, 39, 43, 44, 48, 54, 63, 68, 74, 89, 110];

/// {cmd, group, cmd_ver, notif_ver} of the real file the driver depends on.
pub const CMD_VERSIONS: &[[u8; 4]] = &[
    [0x0d, 1, 15, 0], // SCAN_REQ_UMAC
    [0x0c, 1, 5, 0],  // SCAN_CFG
    [0x08, 1, 4, 0],  // PHY_CONTEXT
    [0x18, 1, 12, 2], // ADD_STA
    [0x1c, 1, 9, 7],  // TX_CMD
    [0x28, 1, 5, 0],  // MAC_CONTEXT
    [0x2b, 1, 2, 1],  // BINDING_CONTEXT
    [0xc8, 1, 0, 5],  // MCC_UPDATE
    [0x02, 0xc, 0, 4], // NVM_GET_INFO
    [0x05, 3, 1, 0],  // SESSION_PROTECTION
    [0x08, 5, 2, 0],  // RLC_CONFIG
    [0x0f, 5, 4, 0],  // TLC_MNG_CONFIG
    [0x17, 5, 3, 2],  // SCD_QUEUE_CONFIG
    [0x01, 2, 2, 0],  // SOC_CONFIGURATION
];

/// Transmit and receive chains A and B (PHY_SKU bits 16-19, 20-23).
pub const PHY_SKU: u32 = 0x0033_0018;

/// Byte `i` of section `k` (LMAC, then UMAC, then paging, counted on).
pub fn byte(k: usize, i: usize) -> u8 {
    ((k * 37 + i * 13) as u8) ^ 0x5a
}

/// The file.
pub fn firmware() -> Vec<u8> {
    let mut f = Vec::new();
    f.extend_from_slice(&0u32.to_le_bytes());
    f.extend_from_slice(&0x0a4c_5749u32.to_le_bytes());
    let mut name = [0u8; 64];
    name[..20].copy_from_slice(b"release/myos-sim::77");
    f.extend_from_slice(&name);
    f.extend_from_slice(&(77u32 << 24).to_le_bytes());
    f.extend_from_slice(&[0; 12]);
    let mut tlv = |ty: u32, data: &[u8]| {
        f.extend_from_slice(&ty.to_le_bytes());
        f.extend_from_slice(&(data.len() as u32).to_le_bytes());
        f.extend_from_slice(data);
        f.resize(f.len().next_multiple_of(4), 0);
    };
    let bits = |set: &[usize], words: usize| {
        (0..words)
            .filter_map(|w| {
                let v = set.iter().filter(|&&b| b / 32 == w).fold(0u32, |a, &b| a | 1 << (b % 32));
                (v != 0).then(|| [(w as u32).to_le_bytes(), v.to_le_bytes()].concat())
            })
            .collect::<Vec<_>>()
    };
    tlv(27, &2u32.to_le_bytes()); // NUM_OF_CPU
    tlv(23, &PHY_SKU.to_le_bytes());
    tlv(6, &512u32.to_le_bytes()); // PROBE_MAX_LEN
    tlv(31, &67u32.to_le_bytes()); // N_SCAN_CHANNELS
    for r in bits(API, 4) {
        tlv(29, &r);
    }
    for r in bits(CAPA, 5) {
        tlv(30, &r);
    }
    tlv(48, &CMD_VERSIONS.concat());
    let mut lmac_debug = [0u8; 32];
    lmac_debug[..4].copy_from_slice(&(LMAC_ERROR_TABLE | 0xc000_0000).to_le_bytes());
    tlv(55, &lmac_debug);
    let section = |k: usize, s: &SimSection| {
        let mut d = s.devoff.to_le_bytes().to_vec();
        d.extend((0..s.len).map(|i| byte(k, i)));
        d
    };
    let mut k = 0;
    for (sections, separator) in [(&LMAC[..], Some(0xffff_ccccu32)), (&UMAC[..], Some(0xaaaa_bbbb)), (&PAGING[..], None)] {
        for s in sections {
            tlv(19, &section(k, s));
            k += 1;
        }
        if let Some(sep) = separator {
            tlv(19, &sep.to_le_bytes());
        }
    }
    f
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fw::Firmware;

    #[test]
    fn parses_as_the_real_one() {
        let file = firmware();
        let fw = Firmware::parse(&file).unwrap();
        assert_eq!((fw.name, fw.version[0]), (&b"release/myos-sim::77"[..], 77));
        assert_eq!((fw.lmac.len(), fw.umac.len(), fw.paging.len()), (2, 2, 2));
        assert_eq!(fw.lmac[1].devoff, 0x0040_8000);
        assert_eq!(fw.lmac[1].data.len(), 5000);
        assert_eq!(fw.umac[0].data[3], byte(2, 3));
        assert_eq!(fw.paging[1].data[99], byte(5, 99));
        assert!(API.iter().all(|&b| fw.api(b)) && CAPA.iter().all(|&b| fw.capa(b)));
        assert!(!fw.capa(12) && !fw.api(68));
        assert_eq!((fw.cmd_ver(1, 0x0d), fw.notif_ver(1, 0x1c)), (15, 7));
        assert_eq!((fw.tx_chains(), fw.rx_chains(), fw.n_scan_channels), (3, 3, 67));
        assert_eq!(fw.lmac_error_table, LMAC_ERROR_TABLE);
    }
}
