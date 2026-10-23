//! The firmware file (struct mtw_ucode, mtw_load_microcode's checks;
//! notices in LICENSE): a header with the lengths of the instruction (ILM)
//! and data (DLM) parts, the interrupt vector block, then the parts.

use crate::reg::MCU_IVB_LEN;

const HDR_LEN: usize = 32;

/// A firmware file, checked.
#[derive(Debug, PartialEq, Eq)]
pub struct Firmware<'a> {
    pub build_ver: u16,
    pub fw_ver: u16,
    /// The interrupt vectors, written last (they start the MCU).
    pub ivb: &'a [u8],
    pub ilm: &'a [u8],
    pub dlm: &'a [u8],
}

impl<'a> Firmware<'a> {
    pub fn parse(file: &'a [u8]) -> Result<Self, &'static str> {
        if file.len() < HDR_LEN + MCU_IVB_LEN {
            return Err("firmware header too short");
        }
        let word = |at: usize| u32::from_le_bytes(file[at..at + 4].try_into().unwrap()) as usize;
        let (ilm_len, dlm_len) = (word(0), word(4));
        // The ILM length counts the vector block.
        if ilm_len < MCU_IVB_LEN || file.len() < HDR_LEN + ilm_len + dlm_len {
            return Err("firmware payload too short");
        }
        let ilm = HDR_LEN + MCU_IVB_LEN;
        let dlm = HDR_LEN + ilm_len;
        Ok(Firmware {
            build_ver: u16::from_le_bytes([file[8], file[9]]),
            fw_ver: u16::from_le_bytes([file[10], file[11]]),
            ivb: &file[HDR_LEN..ilm],
            ilm: &file[ilm..dlm],
            dlm: &file[dlm..dlm + dlm_len],
        })
    }
}

/// A firmware file of these parts (the tests' and the model's).
pub fn build(ivb: &[u8; MCU_IVB_LEN], ilm: &[u8], dlm: &[u8]) -> alloc::vec::Vec<u8> {
    let mut f = alloc::vec![0u8; HDR_LEN];
    f[0..4].copy_from_slice(&((MCU_IVB_LEN + ilm.len()) as u32).to_le_bytes());
    f[4..8].copy_from_slice(&(dlm.len() as u32).to_le_bytes());
    f.extend_from_slice(ivb);
    f.extend_from_slice(ilm);
    f.extend_from_slice(dlm);
    f
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse() {
        let file = build(&[7; MCU_IVB_LEN], &[1, 2, 3, 4], &[]);
        let fw = Firmware::parse(&file).unwrap();
        assert_eq!((fw.ivb, fw.ilm, fw.dlm), (&[7u8; MCU_IVB_LEN][..], &[1u8, 2, 3, 4][..], &[][..]));
        assert!(Firmware::parse(&file[..file.len() - 1]).is_err());
        assert!(Firmware::parse(&file[..40]).is_err());
    }
}
