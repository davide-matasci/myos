//! The device as the driver reaches it: vendor and class requests on its
//! control endpoint, writes to its bulk OUT endpoints, reads of the MCU's
//! answers, and time. The frames' bulk IN endpoint is the daemon's to
//! read (`rx` and `mt76x2::frame` take what it brings).

/// A transfer failed (the device stalled it, did not answer, or is gone).
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub struct UsbError;

pub trait Usb {
    /// A vendor request to the device (`bmRequestType` 0x40) with `data`
    /// as its OUT data stage.
    fn vendor_out(&mut self, request: u8, value: u16, index: u16, data: &[u8]) -> Result<(), UsbError>;
    /// A vendor request from the device (0xc0): `data` filled.
    fn vendor_in(&mut self, request: u8, value: u16, index: u16, data: &mut [u8]) -> Result<(), UsbError>;
    /// A class request to the device (0x20) with `data` as its OUT data
    /// stage (the MT76x2U's MCU takes its ROM patch commands so).
    fn class_out(&mut self, request: u8, value: u16, index: u16, data: &[u8]) -> Result<(), UsbError>;
    /// One bulk OUT transfer on `endpoint`.
    fn bulk_out(&mut self, endpoint: u8, data: &[u8]) -> Result<(), UsbError>;
    /// One bulk IN transfer on `endpoint` into `buf`, waiting up to
    /// `timeout_ms`: its length, 0 when none came.
    fn bulk_in(&mut self, endpoint: u8, buf: &mut [u8], timeout_ms: u32) -> Result<usize, UsbError>;
    /// Wait (the waits here are microseconds to a tenth of a second).
    fn delay_us(&mut self, us: u32);
}

/// The interface's bulk endpoints, as mtw_attach and mt76u_set_endpoints
/// take them: the first IN one brings the frames heard, the second the
/// MCU's answers (the MT76x2U's); the first OUT one takes the MCU's
/// commands (and the MT7601U's management frames), the next four the
/// data frames of each access category (best effort, background, video,
/// voice), and one more.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub struct Endpoints {
    pub rx: u8,
    pub resp: u8,
    pub mcu: u8,
    pub data: [u8; 5],
}

impl Endpoints {
    /// From the bulk endpoints' addresses in the interface's order.
    pub fn new(bulk: &[u8]) -> Option<Self> {
        let mut ins = bulk.iter().filter(|a| *a & 0x80 != 0);
        let mut outs = bulk.iter().filter(|a| *a & 0x80 == 0);
        let rx = *ins.next()?;
        let resp = *ins.next()?;
        let mcu = *outs.next()?;
        let mut data = [0; 5];
        for d in &mut data {
            *d = *outs.next()?;
        }
        Some(Endpoints { rx, resp, mcu, data })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn endpoints() {
        // An MT7601U's: IN 0x84, 0x85, OUT 0x08, then 0x04 to 0x07, 0x09.
        let eps = Endpoints::new(&[0x84, 0x85, 0x08, 0x04, 0x05, 0x06, 0x07, 0x09]).unwrap();
        assert_eq!(eps, Endpoints { rx: 0x84, resp: 0x85, mcu: 0x08, data: [0x04, 0x05, 0x06, 0x07, 0x09] });
        assert_eq!(Endpoints::new(&[0x84, 0x85, 0x08, 0x04]), None);
    }
}
