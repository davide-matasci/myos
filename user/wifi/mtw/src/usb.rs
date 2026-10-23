//! The device as the driver reaches it: vendor requests on its control
//! endpoint, writes to its bulk OUT endpoints, and time. The bulk IN
//! endpoint is the daemon's to read (`rx` takes what it brings).

/// A transfer failed (the device stalled it, did not answer, or is gone).
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub struct UsbError;

pub trait Usb {
    /// A vendor request to the device (`bmRequestType` 0x40) with `data`
    /// as its OUT data stage.
    fn vendor_out(&mut self, request: u8, value: u16, index: u16, data: &[u8]) -> Result<(), UsbError>;
    /// A vendor request from the device (0xc0): `data` filled.
    fn vendor_in(&mut self, request: u8, value: u16, index: u16, data: &mut [u8]) -> Result<(), UsbError>;
    /// One bulk OUT transfer on `endpoint`.
    fn bulk_out(&mut self, endpoint: u8, data: &[u8]) -> Result<(), UsbError>;
    /// Wait (the waits here are microseconds to a tenth of a second).
    fn delay_us(&mut self, us: u32);
}

/// The interface's bulk endpoints, as mtw_attach takes them: the first IN
/// one brings the frames heard (the second the MCU's answers, unused);
/// the first OUT one takes the MCU's commands and the management frames,
/// the next four the data frames of each access category (and one more).
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub struct Endpoints {
    pub rx: u8,
    pub mcu: u8,
    pub data: [u8; 5],
}

impl Endpoints {
    /// From the bulk endpoints' addresses in the interface's order.
    pub fn new(bulk: &[u8]) -> Option<Self> {
        let mut ins = bulk.iter().filter(|a| *a & 0x80 != 0);
        let mut outs = bulk.iter().filter(|a| *a & 0x80 == 0);
        let rx = *ins.next()?;
        ins.next()?;
        let mcu = *outs.next()?;
        let mut data = [0; 5];
        for d in &mut data {
            *d = *outs.next()?;
        }
        Some(Endpoints { rx, mcu, data })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn endpoints() {
        // An MT7601U's: IN 0x84, 0x85, OUT 0x08, then 0x04 to 0x07, 0x09.
        let eps = Endpoints::new(&[0x84, 0x85, 0x08, 0x04, 0x05, 0x06, 0x07, 0x09]).unwrap();
        assert_eq!(eps, Endpoints { rx: 0x84, mcu: 0x08, data: [0x04, 0x05, 0x06, 0x07, 0x09] });
        assert_eq!(Endpoints::new(&[0x84, 0x85, 0x08, 0x04]), None);
    }
}
