//! The MT76x2U's two files (struct mt76x02_fw_header, mt76x02_patch_header
//! and mt76x2u_mcu_load_firmware's checks; notices in LICENSE): the ROM
//! patch, a 30-byte header then the patch; the firmware, a header with
//! the lengths of its instruction (ILM) and data (DLM) parts, then the
//! parts, the interrupt vectors at the start of the ILM.

const HDR_LEN: usize = 32;
const PATCH_HDR_LEN: usize = 30;

/// A firmware file, checked.
#[derive(Debug, PartialEq, Eq)]
pub struct Firmware<'a> {
    pub build_ver: u16,
    pub fw_ver: u16,
    pub ilm: &'a [u8],
    pub dlm: &'a [u8],
}

impl<'a> Firmware<'a> {
    pub fn parse(file: &'a [u8]) -> Result<Self, &'static str> {
        if file.len() < HDR_LEN {
            return Err("firmware header too short");
        }
        let word = |at: usize| u32::from_le_bytes(file[at..at + 4].try_into().unwrap()) as usize;
        let (ilm_len, dlm_len) = (word(0), word(4));
        if file.len() != HDR_LEN + ilm_len + dlm_len {
            return Err("firmware not the length its header says");
        }
        Ok(Firmware {
            build_ver: u16::from_le_bytes([file[8], file[9]]),
            fw_ver: u16::from_le_bytes([file[10], file[11]]),
            ilm: &file[HDR_LEN..HDR_LEN + ilm_len],
            dlm: &file[HDR_LEN + ilm_len..],
        })
    }
}

/// A ROM patch file, checked: its build time (16 characters) and the
/// patch.
#[derive(Debug, PartialEq, Eq)]
pub struct RomPatch<'a> {
    pub build_time: &'a [u8],
    pub patch: &'a [u8],
}

impl<'a> RomPatch<'a> {
    pub fn parse(file: &'a [u8]) -> Result<Self, &'static str> {
        if file.len() <= PATCH_HDR_LEN {
            return Err("ROM patch too short");
        }
        Ok(RomPatch { build_time: &file[..16], patch: &file[PATCH_HDR_LEN..] })
    }
}

/// The files of these parts (the tests' and the model's).
pub fn build(ilm: &[u8], dlm: &[u8]) -> alloc::vec::Vec<u8> {
    let mut f = alloc::vec![0u8; HDR_LEN];
    f[0..4].copy_from_slice(&(ilm.len() as u32).to_le_bytes());
    f[4..8].copy_from_slice(&(dlm.len() as u32).to_le_bytes());
    f[8..10].copy_from_slice(&0x0001u16.to_le_bytes());
    f[16..32].copy_from_slice(b"201507311614____");
    f.extend_from_slice(ilm);
    f.extend_from_slice(dlm);
    f
}

pub fn build_patch(patch: &[u8]) -> alloc::vec::Vec<u8> {
    let mut f = alloc::vec![0u8; PATCH_HDR_LEN];
    f[..16].copy_from_slice(b"20141115060606a\n");
    f[16..20].copy_from_slice(b"ALPS");
    f.extend_from_slice(patch);
    f
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse() {
        let file = build(&[1, 2, 3, 4, 5], &[6, 7]);
        let fw = Firmware::parse(&file).unwrap();
        assert_eq!((fw.ilm, fw.dlm, fw.build_ver), (&[1u8, 2, 3, 4, 5][..], &[6u8, 7][..], 1));
        assert!(Firmware::parse(&file[..file.len() - 1]).is_err());
        assert!(Firmware::parse(&[file.clone(), alloc::vec![0]].concat()).is_err());
        assert!(Firmware::parse(&file[..20]).is_err());
        let p = build_patch(&[9; 100]);
        assert_eq!(RomPatch::parse(&p).unwrap().patch, &[9u8; 100][..]);
        assert!(RomPatch::parse(&p[..30]).is_err());
    }
}
