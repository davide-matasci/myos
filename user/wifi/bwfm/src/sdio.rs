//! The card as the driver reaches it: single bytes (CMD52) and transfers
//! (CMD53) on its functions, the functions' block sizes, and time. The
//! card's interrupt is the daemon's to wait for; it then asks the driver
//! to service the card.

/// A transfer failed (the card did not answer, or said no).
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub struct SdioError;

pub trait Sdio {
    /// Register `addr` of function `func` (CMD52).
    fn read_1(&mut self, func: u8, addr: u32) -> Result<u8, SdioError>;
    fn write_1(&mut self, func: u8, addr: u32, value: u8) -> Result<(), SdioError>;
    /// `buf` from register `addr` of function `func` (CMD53), the address
    /// incrementing with each byte or fixed (a FIFO). Whole 32-bit words.
    fn read(&mut self, func: u8, addr: u32, buf: &mut [u8], incr: bool) -> Result<(), SdioError>;
    fn write(&mut self, func: u8, addr: u32, data: &[u8], incr: bool) -> Result<(), SdioError>;
    /// Function `func`'s block size for the transfers that are a whole
    /// number of blocks.
    fn set_block_size(&mut self, func: u8, size: u16) -> Result<(), SdioError>;
    /// Wait (microseconds to a few milliseconds).
    fn delay_us(&mut self, us: u32);
    /// A clock, in milliseconds (for the waits on the firmware).
    fn now_ms(&mut self) -> u64;
}
