//! The SD card side, the same behind every controller: bringing a card up
//! (identification, its address, its size, the 4-bit bus) and moving blocks
//! (SD Physical Layer Simplified Specification, the commands and registers
//! as the `sdio-host` crate has them).

use sdio_host::common_cmd::{self, Cmd, R1, R2, R3, Resp, Rz};
use sdio_host::sd::{CID, CSD, CardStatus, CurrentState, OCR, RCA, SD};
use sdio_host::sd_cmd::{self, R6, R7};

/// The block size every card here uses (an SDSC card is told with CMD16).
pub const BLOCK: usize = 512;
/// The identification clock, at most 400 kHz.
const CLOCK_IDENT: u32 = 400_000;
/// The default-speed clock every card takes.
const CLOCK_DATA: u32 = 25_000_000;

/// What a command answers, as a controller is told it.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Response {
    None,
    /// 48 bits, CRC and index checked (R1, R6, R7).
    Short,
    /// 48 bits without a CRC (R3, the OCR).
    ShortNoCrc,
    /// R1 with the card busy on DAT0 after it (R1b).
    ShortBusy,
    /// 136 bits (R2: the CID, the CSD).
    Long,
}

/// The response of the `sdio-host` command types.
pub trait Kind {
    const RESPONSE: Response;
}
impl Kind for Rz {
    const RESPONSE: Response = Response::None;
}
impl Kind for R1 {
    const RESPONSE: Response = Response::Short;
}
impl Kind for R2 {
    const RESPONSE: Response = Response::Long;
}
impl Kind for R3 {
    const RESPONSE: Response = Response::ShortNoCrc;
}
impl Kind for R6 {
    const RESPONSE: Response = Response::Short;
}
impl Kind for R7 {
    const RESPONSE: Response = Response::Short;
}

/// The data a command moves: whole blocks, read into `buf` or written
/// from it.
pub struct Data<'a> {
    pub buf: &'a mut [u8],
    pub write: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    /// No response, or no data, in time.
    Timeout,
    /// A CRC, index or end-bit error, or the controller's own.
    Bus,
    /// The card says the command failed (its status's error bits).
    Card,
}

/// A controller with one card slot.
pub trait Host {
    /// Its name in the disk's description (`sdhci`, `sdhost`).
    fn name(&self) -> &'static str;
    /// The bus clock: at most `hz`.
    fn set_clock(&mut self, hz: u32);
    /// The 4-bit bus (else 1-bit).
    fn set_wide(&mut self, wide: bool);
    /// Send command `index` with `arg` and wait for its response: the
    /// response's bits, `[31:0]` first, a long one's whole 128 (the CRC's
    /// byte as 0); `data`'s blocks moved after it.
    fn command(&mut self, index: u8, arg: u32, response: Response, data: Option<Data>) -> Result<[u32; 4], Error>;
}

/// A card that came up.
pub struct Card {
    rca: u16,
    /// SDHC/SDXC: addressed by block, not by byte.
    high_capacity: bool,
    /// In 512-byte sectors.
    pub capacity: u64,
    /// The CID's product name and serial number (`/dev/<disk>/info`).
    pub product: [u8; 5],
    pub serial: u32,
}

fn run<R: Resp + Kind>(host: &mut dyn Host, cmd: Cmd<R>) -> Result<[u32; 4], Error> {
    host.command(cmd.cmd, cmd.arg, R::RESPONSE, None)
}

/// [`run`] for an R1b command.
fn run_busy(host: &mut dyn Host, cmd: Cmd<R1>) -> Result<[u32; 4], Error> {
    host.command(cmd.cmd, cmd.arg, Response::ShortBusy, None)
}

/// An R1's error bits: the command failed.
fn checked(resp: [u32; 4]) -> Result<CardStatus<SD>, Error> {
    let s = CardStatus::<SD>::from(resp[0]);
    let failed = s.out_of_range()
        || s.address_error()
        || s.block_len_error()
        || s.wp_violation()
        || s.com_crc_error()
        || s.illegal_command()
        || s.card_ecc_failed()
        || s.cc_error()
        || s.error();
    if failed { Err(Error::Card) } else { Ok(s) }
}

fn long(resp: [u32; 4]) -> u128 {
    resp.iter().rev().fold(0u128, |v, &w| (v << 32) | u128::from(w))
}

impl Card {
    /// Bring up the card behind `host`, from the idle state: `None` when
    /// there is none or it does not answer as an SD memory card does.
    /// `now_ms` is the monotonic clock, for the card's own initialization.
    pub fn init(host: &mut dyn Host, now_ms: &dyn Fn() -> u64) -> Option<Card> {
        host.set_wide(false);
        host.set_clock(CLOCK_IDENT);
        run(host, common_cmd::idle()).ok()?;
        // CMD8: a card of version 2.00 or later echoes the pattern, and
        // only those can be high capacity.
        let v2 = run(host, sd_cmd::send_if_cond(1, 0xaa)).is_ok_and(|r| r[0] & 0xfff == 0x1aa);
        // ACMD41 until the card is no longer busy powering up (at most a
        // second, the specification says).
        let start = now_ms();
        let ocr = loop {
            run(host, common_cmd::app_cmd(0)).ok()?;
            let ocr = OCR::<SD>::from(run(host, sd_cmd::sd_send_op_cond(v2, false, false, 0x1ff)).ok()?[0]);
            if !ocr.is_busy() {
                break ocr;
            }
            if now_ms() - start > 1000 {
                return None;
            }
        };
        let cid = CID::<SD>::from(long(run(host, common_cmd::all_send_cid()).ok()?));
        let rca = RCA::<SD>::from(run(host, sd_cmd::send_relative_address()).ok()?[0]).address();
        let csd = CSD::<SD>::from(long(run(host, common_cmd::send_csd(rca)).ok()?));
        checked(run_busy(host, common_cmd::select_card(rca)).ok()?).ok()?;
        // Every SD memory card has the 4-bit bus.
        run(host, common_cmd::app_cmd(rca)).ok()?;
        checked(run(host, sd_cmd::set_bus_width(true)).ok()?).ok()?;
        host.set_wide(true);
        checked(run(host, common_cmd::set_block_length(BLOCK as u32)).ok()?).ok()?;
        host.set_clock(CLOCK_DATA);
        let mut product = [0u8; 5];
        product.copy_from_slice(cid.product_name().as_bytes().get(..5).unwrap_or(b"     "));
        Some(Card {
            rca,
            high_capacity: ocr.high_capacity(),
            capacity: csd.card_size() / BLOCK as u64,
            product,
            serial: cid.serial(),
        })
    }

    /// Read (`write` false) or write `buf`, whole blocks from block `lba`,
    /// in one command (two blocks or more: a multiple-block one, stopped
    /// with CMD12).
    pub fn transfer(&self, host: &mut dyn Host, lba: u64, buf: &mut [u8], write: bool) -> Result<(), Error> {
        let blocks = buf.len() / BLOCK;
        let addr = if self.high_capacity { lba } else { lba * BLOCK as u64 };
        let addr = u32::try_from(addr).map_err(|_| Error::Card)?;
        let cmd = match (write, blocks > 1) {
            (false, false) => common_cmd::read_single_block(addr),
            (false, true) => common_cmd::read_multiple_blocks(addr),
            (true, false) => common_cmd::write_single_block(addr),
            (true, true) => common_cmd::write_multiple_blocks(addr),
        };
        let done = host.command(cmd.cmd, cmd.arg, Response::Short, Some(Data { buf, write })).and_then(checked);
        if blocks > 1 {
            // Stopped even after an error: the card must leave the data
            // state for the next command.
            let stop = run_busy(host, common_cmd::stop_transmission());
            done?;
            stop?;
        } else {
            done?;
        }
        if write { self.wait_ready(host) } else { Ok(()) }
    }

    /// Until the card has programmed what it was sent: back in the
    /// transfer state, ready for data.
    fn wait_ready(&self, host: &mut dyn Host) -> Result<(), Error> {
        for _ in 0..1_000_000 {
            let status = checked(run(host, common_cmd::card_status(self.rca, false))?)?;
            if status.ready_for_data() && status.state() == CurrentState::Transfer {
                return Ok(());
            }
        }
        Err(Error::Timeout)
    }
}
