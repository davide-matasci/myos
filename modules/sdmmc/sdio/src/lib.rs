//! The SD bus as the `sdmmc` module's controller drivers offer it ([`Bus`]:
//! a command with its response and data, the clock and the bus width),
//! and an SDIO card brought up on it ([`Io`]: the SDIO Simplified
//! Specification's CMD5, CMD52 and CMD53, the CCCR, a function's FBR and
//! the card's CIS). `no_std` and without a controller, so the card layer
//! runs against a model on the host (`cargo test -p sdio`); the SD memory
//! card layer stays in the module, on the `sdio-host` crate.

#![no_std]

/// What a command answers, as a controller is told it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Response {
    None,
    /// 48 bits, CRC and index checked (R1, R5, R6, R7).
    Short,
    /// 48 bits without a CRC (R3, R4: an OCR).
    ShortNoCrc,
    /// R1 with the card busy on DAT0 after it (R1b).
    ShortBusy,
    /// 136 bits (R2: the CID, the CSD).
    Long,
}

/// A command's data: `buf` moved after it in blocks of `block` bytes (a
/// whole number of them; one of less than 512 for an SDIO byte-mode
/// transfer), to the card when `write`.
pub struct Data<'a> {
    pub buf: &'a mut [u8],
    pub write: bool,
    pub block: usize,
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
pub trait Bus {
    /// Its name in a device's description (`sdhci`, `sdhost`).
    fn name(&self) -> &'static str;
    /// The bus clock: at most `hz`.
    fn set_clock(&mut self, hz: u32);
    /// The 4-bit bus (else 1-bit).
    fn set_wide(&mut self, wide: bool);
    /// Send command `index` with `arg` and wait for its response: the
    /// response's bits, `[31:0]` first, a long one's whole 128 (the CRC's
    /// byte as 0); `data`'s blocks moved after it.
    fn command(&mut self, index: u8, arg: u32, response: Response, data: Option<Data>) -> Result<[u32; 4], Error>;
    /// Wait `ms` milliseconds (a card powering up).
    fn delay_ms(&mut self, ms: u32);
}

/// The clock while a card is identified, and once it is up (the default
/// speed; high speed doubles it).
pub const CLOCK_IDENT: u32 = 400_000;
pub const CLOCK_DEFAULT: u32 = 25_000_000;
pub const CLOCK_HIGH: u32 = 50_000_000;

/// The commands.
pub mod cmd {
    pub const GO_IDLE: u8 = 0;
    pub const SEND_RELATIVE_ADDR: u8 = 3;
    pub const IO_SEND_OP_COND: u8 = 5;
    pub const SELECT_CARD: u8 = 7;
    pub const IO_RW_DIRECT: u8 = 52;
    pub const IO_RW_EXTENDED: u8 = 53;

    /// CMD52's argument: register `addr` of function `func`, read, or
    /// written with `data` (`read_after`: the register's new value comes
    /// back in the response).
    pub fn rw_direct(write: bool, func: u8, addr: u32, data: u8, read_after: bool) -> u32 {
        u32::from(write) << 31
            | u32::from(func & 7) << 28
            | u32::from(write && read_after) << 27
            | (addr & 0x1ffff) << 9
            | u32::from(data)
    }

    /// CMD53's argument: `count` bytes (byte mode, 1 to 512) or blocks
    /// (block mode) at register `addr` of function `func`, the address
    /// incrementing with each byte (`incr`) or fixed (a FIFO).
    pub fn rw_extended(write: bool, func: u8, addr: u32, incr: bool, block_mode: bool, count: usize) -> u32 {
        u32::from(write) << 31
            | u32::from(func & 7) << 28
            | u32::from(block_mode) << 27
            | u32::from(incr) << 26
            | (addr & 0x1ffff) << 9
            | (count as u32 & 0x1ff)
    }
}

/// The card common control registers (function 0) and a function's FBR.
pub mod cccr {
    /// CCCR and SDIO revision.
    pub const REVISION: u32 = 0x00;
    /// I/O enable (bit per function), I/O ready, interrupt enable (bit 0
    /// the master enable).
    pub const IO_ENABLE: u32 = 0x02;
    pub const IO_READY: u32 = 0x03;
    pub const INT_ENABLE: u32 = 0x04;
    pub const INT_PENDING: u32 = 0x05;
    /// I/O abort: function in bits 2:0, bit 3 resets the I/O part.
    pub const IO_ABORT: u32 = 0x06;
    pub const IO_ABORT_RESET: u8 = 1 << 3;
    /// Bus interface control: the width in bits 1:0.
    pub const BUS_INTERFACE: u32 = 0x07;
    pub const BUS_WIDTH_1: u8 = 0;
    pub const BUS_WIDTH_4: u8 = 2;
    /// Card capability: low-speed card (bit 6) and its 4-bit support (7).
    pub const CAPABILITY: u32 = 0x08;
    pub const CAP_LOW_SPEED: u8 = 1 << 6;
    pub const CAP_LOW_SPEED_4BIT: u8 = 1 << 7;
    /// Function 0's CIS pointer, 3 bytes little endian.
    pub const CIS_POINTER: u32 = 0x09;
    /// Bus speed select: high speed supported (bit 0), enabled (bit 1).
    pub const SPEED: u32 = 0x13;
    pub const SPEED_SHS: u8 = 1 << 0;
    pub const SPEED_EHS: u8 = 1 << 1;

    /// Function `f`'s basic registers: its block size at `+0x10` (2 bytes
    /// little endian), its CIS pointer at `+0x09`.
    pub fn fbr(f: u8) -> u32 {
        u32::from(f) * 0x100
    }
    pub const FBR_CIS_POINTER: u32 = 0x09;
    pub const FBR_BLOCK_SIZE: u32 = 0x10;

    /// Where the CIS may be.
    pub const CIS_START: u32 = 0x1000;
    pub const CIS_END: u32 = 0x18000;
    /// The tuples read.
    pub const CISTPL_NULL: u8 = 0x00;
    pub const CISTPL_MANFID: u8 = 0x20;
    pub const CISTPL_END: u8 = 0xff;
}

/// CMD5's response: the card's power-up state and what it has.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct R4 {
    pub ready: bool,
    pub functions: u8,
    pub memory_present: bool,
    /// The voltage window, bits 23:0 as in an OCR.
    pub ocr: u32,
}

impl From<u32> for R4 {
    fn from(v: u32) -> Self {
        R4 { ready: v >> 31 != 0, functions: ((v >> 28) & 7) as u8, memory_present: v & (1 << 27) != 0, ocr: v & 0x00ff_ffff }
    }
}

/// CMD52's and CMD53's response: the error flags and, for CMD52, the byte.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct R5 {
    pub flags: u8,
    pub data: u8,
}

impl R5 {
    /// COM_CRC_ERROR, ILLEGAL_COMMAND, ERROR, FUNCTION_NUMBER,
    /// OUT_OF_RANGE: the command did not do what was asked.
    pub const ERRORS: u8 = 0x80 | 0x40 | 0x08 | 0x02 | 0x01;

    pub fn checked(resp: [u32; 4]) -> Result<R5, Error> {
        let r = R5 { flags: (resp[0] >> 8) as u8, data: resp[0] as u8 };
        if r.flags & R5::ERRORS != 0 { Err(Error::Card) } else { Ok(r) }
    }
}

/// The most functions a card has (1 to 7, 0 the CCCR).
pub const MAX_FUNCTIONS: usize = 8;
/// A byte-mode CMD53 moves this much at most.
pub const BYTE_MODE_MAX: usize = 512;

/// An SDIO card that came up: selected, on the 4-bit bus where it has
/// one, at high speed where it supports it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Io {
    pub rca: u16,
    /// Its I/O functions, 1 to 7.
    pub functions: u8,
    pub memory_present: bool,
    /// From the CIS's MANFID tuple (0 when it has none).
    pub manufacturer: u16,
    pub product: u16,
    pub wide: bool,
    pub high_speed: bool,
    /// Each function's block size for CMD53's block mode, 0 until set.
    pub block: [u16; MAX_FUNCTIONS],
}

impl Io {
    /// Bring up the SDIO card behind `bus`, from power on; `Err(Timeout)`
    /// when nothing answers CMD5 (no card, or an SD memory card).
    pub fn init(bus: &mut dyn Bus) -> Result<Io, Error> {
        bus.set_wide(false);
        bus.set_clock(CLOCK_IDENT);
        // Reset the I/O part of a card that is up already (nothing there
        // does not answer: not an error), then the memory part's idle.
        let _ = bus.command(cmd::IO_RW_DIRECT, cmd::rw_direct(true, 0, cccr::IO_ABORT, cccr::IO_ABORT_RESET, false), Response::Short, None);
        bus.command(cmd::GO_IDLE, 0, Response::None, None)?;
        let r4 = R4::from(bus.command(cmd::IO_SEND_OP_COND, 0, Response::ShortNoCrc, None)?[0]);
        if r4.functions == 0 {
            return Err(Error::Card);
        }
        // Our 3.3 V within the card's window, until it is ready (a second).
        let ocr = r4.ocr & 0x00ff_8000;
        let mut r4 = r4;
        for _ in 0..100 {
            r4 = R4::from(bus.command(cmd::IO_SEND_OP_COND, ocr, Response::ShortNoCrc, None)?[0]);
            if r4.ready {
                break;
            }
            bus.delay_ms(10);
        }
        if !r4.ready {
            return Err(Error::Timeout);
        }
        let rca = (bus.command(cmd::SEND_RELATIVE_ADDR, 0, Response::Short, None)?[0] >> 16) as u16;
        bus.command(cmd::SELECT_CARD, u32::from(rca) << 16, Response::ShortBusy, None)?;
        let mut io = Io {
            rca,
            functions: r4.functions,
            memory_present: r4.memory_present,
            manufacturer: 0,
            product: 0,
            wide: false,
            high_speed: false,
            block: [0; MAX_FUNCTIONS],
        };
        // The 4-bit bus, unless a low-speed card without it.
        let cap = io.read_byte(bus, 0, cccr::CAPABILITY)?;
        if cap & cccr::CAP_LOW_SPEED == 0 || cap & cccr::CAP_LOW_SPEED_4BIT != 0 {
            let v = io.read_byte(bus, 0, cccr::BUS_INTERFACE)? & !3;
            io.write_byte(bus, 0, cccr::BUS_INTERFACE, v | cccr::BUS_WIDTH_4)?;
            bus.set_wide(true);
            io.wide = true;
        }
        // High speed where the card has it.
        let speed = io.read_byte(bus, 0, cccr::SPEED)?;
        if speed & cccr::SPEED_SHS != 0 {
            io.write_byte(bus, 0, cccr::SPEED, speed | cccr::SPEED_EHS)?;
            io.high_speed = true;
        }
        bus.set_clock(if io.high_speed { CLOCK_HIGH } else { CLOCK_DEFAULT });
        if let Some((m, p)) = io.manfid(bus)? {
            io.manufacturer = m;
            io.product = p;
        }
        Ok(io)
    }

    /// Register `addr` of function `func` (CMD52).
    pub fn read_byte(&self, bus: &mut dyn Bus, func: u8, addr: u32) -> Result<u8, Error> {
        let resp = bus.command(cmd::IO_RW_DIRECT, cmd::rw_direct(false, func, addr, 0, false), Response::Short, None)?;
        Ok(R5::checked(resp)?.data)
    }

    pub fn write_byte(&self, bus: &mut dyn Bus, func: u8, addr: u32, value: u8) -> Result<(), Error> {
        let resp = bus.command(cmd::IO_RW_DIRECT, cmd::rw_direct(true, func, addr, value, false), Response::Short, None)?;
        R5::checked(resp).map(|_| ())
    }

    /// Move `buf` at register `addr` of function `func` (CMD53), the
    /// address incrementing with each byte or fixed (`incr`): in block
    /// mode when `buf` is a whole number of the function's blocks and
    /// more than one, else in byte mode ([`BYTE_MODE_MAX`] at most).
    pub fn transfer(&self, bus: &mut dyn Bus, func: u8, addr: u32, incr: bool, buf: &mut [u8], write: bool) -> Result<(), Error> {
        let len = buf.len();
        if len == 0 {
            return Ok(());
        }
        let block = usize::from(self.block[usize::from(func & 7)]);
        let (block_mode, count, size) = if block != 0 && len > block && len % block == 0 {
            (true, len / block, block)
        } else if len <= BYTE_MODE_MAX {
            (false, len, len)
        } else {
            return Err(Error::Card);
        };
        // 512 bytes are written as 0.
        let arg = cmd::rw_extended(write, func, addr, incr, block_mode, if block_mode { count } else { count % 512 });
        let resp = bus.command(cmd::IO_RW_EXTENDED, arg, Response::Short, Some(Data { buf, write, block: size }))?;
        R5::checked(resp).map(|_| ())
    }

    /// Function `func`'s block size for block-mode transfers: into its FBR
    /// and remembered here.
    pub fn set_block_size(&mut self, bus: &mut dyn Bus, func: u8, size: u16) -> Result<(), Error> {
        if func == 0 || usize::from(func) >= MAX_FUNCTIONS || size == 0 || size > 2048 {
            return Err(Error::Card);
        }
        let reg = cccr::fbr(func) + cccr::FBR_BLOCK_SIZE;
        self.write_byte(bus, 0, reg, size as u8)?;
        self.write_byte(bus, 0, reg + 1, (size >> 8) as u8)?;
        self.block[usize::from(func)] = size;
        Ok(())
    }

    /// Function 0's CIS: its MANFID tuple's manufacturer and product, if
    /// it has one.
    fn manfid(&self, bus: &mut dyn Bus) -> Result<Option<(u16, u16)>, Error> {
        let mut at = 0;
        for i in 0..3 {
            at |= u32::from(self.read_byte(bus, 0, cccr::CIS_POINTER + i)?) << (8 * i);
        }
        if !(cccr::CIS_START..cccr::CIS_END - 16).contains(&at) {
            return Ok(None);
        }
        // A tuple is its code, its length, its body; at most the CIS.
        while at < cccr::CIS_END {
            let code = self.read_byte(bus, 0, at)?;
            at += 1;
            match code {
                cccr::CISTPL_END => return Ok(None),
                cccr::CISTPL_NULL => continue,
                _ => {}
            }
            let len = u32::from(self.read_byte(bus, 0, at)?);
            at += 1;
            if code == cccr::CISTPL_MANFID && len >= 4 {
                let mut v = [0u8; 4];
                for (i, b) in v.iter_mut().enumerate() {
                    *b = self.read_byte(bus, 0, at + i as u32)?;
                }
                return Ok(Some((u16::from_le_bytes([v[0], v[1]]), u16::from_le_bytes([v[2], v[3]]))));
            }
            at += len;
        }
        Ok(None)
    }
}

#[cfg(test)]
extern crate std;

#[cfg(test)]
mod tests {
    use std::vec;
    use std::vec::Vec;

    use super::*;

    /// A model of an SDIO card with two functions on a bus that records
    /// what it is told: the CCCR, the FBRs and the CIS as 8-bit registers
    /// of function 0, function 1 as 64 KiB of memory (incrementing) with
    /// a FIFO at 0 (fixed: every byte lands in the same place).
    struct Model {
        state: State,
        clock: u32,
        wide: bool,
        f0: [u8; 0x2000],
        f1: Vec<u8>,
        fifo: Vec<u8>,
        /// (index, arg) of every command.
        log: Vec<(u8, u32)>,
        powered_at: usize,
    }

    #[derive(PartialEq, Eq, Debug, Clone, Copy)]
    enum State {
        Idle,
        Ready,
        Addressed,
        Selected,
    }

    const RCA: u16 = 0x0001;

    impl Model {
        fn new() -> Model {
            let mut f0 = [0u8; 0x2000];
            f0[cccr::REVISION as usize] = 0x43;
            f0[cccr::CAPABILITY as usize] = 0x00;
            f0[cccr::SPEED as usize] = cccr::SPEED_SHS;
            // The CIS at 0x1000: a NULL, then MANFID 02d0:a9bf, then END.
            f0[cccr::CIS_POINTER as usize] = 0x00;
            f0[cccr::CIS_POINTER as usize + 1] = 0x10;
            f0[0x1000..0x1008].copy_from_slice(&[cccr::CISTPL_NULL, cccr::CISTPL_MANFID, 4, 0xd0, 0x02, 0xbf, 0xa9, cccr::CISTPL_END]);
            Model { state: State::Idle, clock: 0, wide: false, f0, f1: vec![0; 0x10000], fifo: Vec::new(), log: Vec::new(), powered_at: 0 }
        }

        fn reg(&mut self, func: u8, addr: u32) -> &mut u8 {
            match func {
                0 => &mut self.f0[addr as usize],
                1 => &mut self.f1[addr as usize],
                _ => panic!("function {func}"),
            }
        }
    }

    impl Bus for Model {
        fn name(&self) -> &'static str {
            "model"
        }
        fn set_clock(&mut self, hz: u32) {
            self.clock = hz;
        }
        fn set_wide(&mut self, wide: bool) {
            self.wide = wide;
        }
        fn delay_ms(&mut self, _ms: u32) {}
        fn command(&mut self, index: u8, arg: u32, response: Response, data: Option<Data>) -> Result<[u32; 4], Error> {
            self.log.push((index, arg));
            match index {
                cmd::GO_IDLE => {
                    assert_eq!(response, Response::None);
                    self.state = State::Idle;
                    Ok([0; 4])
                }
                cmd::IO_SEND_OP_COND => {
                    assert_eq!(response, Response::ShortNoCrc);
                    // Two functions, no memory, 3.2 to 3.4 V; ready at the
                    // third ask with a voltage.
                    let mut r = 2 << 28 | 0x0030_0000;
                    if arg != 0 {
                        self.powered_at += 1;
                        if self.powered_at >= 3 {
                            r |= 1 << 31;
                            self.state = State::Ready;
                        }
                    }
                    Ok([r, 0, 0, 0])
                }
                cmd::SEND_RELATIVE_ADDR => {
                    assert_eq!(self.state, State::Ready);
                    self.state = State::Addressed;
                    Ok([u32::from(RCA) << 16, 0, 0, 0])
                }
                cmd::SELECT_CARD => {
                    assert_eq!(arg >> 16, u32::from(RCA));
                    assert_eq!(response, Response::ShortBusy);
                    self.state = State::Selected;
                    Ok([0; 4])
                }
                cmd::IO_RW_DIRECT => {
                    let (write, func, addr) = (arg >> 31 != 0, (arg >> 28) as u8 & 7, (arg >> 9) & 0x1ffff);
                    if self.state != State::Selected {
                        // A reset of a card that is not up: no answer.
                        return Err(Error::Timeout);
                    }
                    if func > 1 {
                        return Ok([0x02 << 8, 0, 0, 0]);
                    }
                    let r = self.reg(func, addr);
                    if write {
                        *r = arg as u8;
                    }
                    Ok([u32::from(*r), 0, 0, 0])
                }
                cmd::IO_RW_EXTENDED => {
                    let d = data.expect("CMD53 moves data");
                    let (write, func, block_mode) = (arg >> 31 != 0, (arg >> 28) as u8 & 7, arg & (1 << 27) != 0);
                    let (incr, addr, count) = (arg & (1 << 26) != 0, (arg >> 9) & 0x1ffff, (arg & 0x1ff) as usize);
                    assert_eq!(d.write, write);
                    assert_eq!(func, 1);
                    let bytes = if block_mode {
                        let block = u16::from_le_bytes([self.f0[0x110], self.f0[0x111]]) as usize;
                        assert_eq!(d.block, block);
                        count * block
                    } else {
                        let n = if count == 0 { 512 } else { count };
                        assert_eq!(d.block, n);
                        n
                    };
                    assert_eq!(d.buf.len(), bytes);
                    for (i, b) in d.buf.iter_mut().enumerate() {
                        if incr {
                            let r = &mut self.f1[addr as usize + i];
                            if write { *r = *b } else { *b = *r }
                        } else if write {
                            self.fifo.push(*b);
                        } else {
                            *b = self.fifo.get(i).copied().unwrap_or(0xee);
                        }
                    }
                    Ok([0; 4])
                }
                _ => Err(Error::Timeout),
            }
        }
    }

    #[test]
    fn brings_the_card_up() {
        let mut m = Model::new();
        let io = Io::init(&mut m).unwrap();
        assert_eq!((io.rca, io.functions, io.memory_present), (RCA, 2, false));
        assert_eq!((io.manufacturer, io.product), (0x02d0, 0xa9bf));
        assert!(io.wide && io.high_speed);
        assert!(m.wide);
        assert_eq!(m.clock, CLOCK_HIGH);
        assert_eq!(m.f0[cccr::BUS_INTERFACE as usize] & 3, cccr::BUS_WIDTH_4);
        assert_eq!(m.f0[cccr::SPEED as usize], cccr::SPEED_SHS | cccr::SPEED_EHS);
        // The I/O reset, idle, CMD5 without and with a voltage.
        assert_eq!(m.log[0].0, cmd::IO_RW_DIRECT);
        assert_eq!(m.log[1], (cmd::GO_IDLE, 0));
        assert_eq!(m.log[2], (cmd::IO_SEND_OP_COND, 0));
        assert_eq!(m.log[3], (cmd::IO_SEND_OP_COND, 0x0030_0000));
        assert_eq!(m.state, State::Selected);
    }

    #[test]
    fn a_low_speed_card_stays_narrow_and_slow() {
        let mut m = Model::new();
        m.f0[cccr::CAPABILITY as usize] = cccr::CAP_LOW_SPEED;
        m.f0[cccr::SPEED as usize] = 0;
        let io = Io::init(&mut m).unwrap();
        assert!(!io.wide && !io.high_speed);
        assert!(!m.wide);
        assert_eq!(m.clock, CLOCK_DEFAULT);
    }

    #[test]
    fn no_card_is_a_timeout() {
        struct Nothing;
        impl Bus for Nothing {
            fn name(&self) -> &'static str {
                "nothing"
            }
            fn set_clock(&mut self, _: u32) {}
            fn set_wide(&mut self, _: bool) {}
            fn delay_ms(&mut self, _: u32) {}
            fn command(&mut self, index: u8, _: u32, _: Response, _: Option<Data>) -> Result<[u32; 4], Error> {
                if index == cmd::GO_IDLE { Ok([0; 4]) } else { Err(Error::Timeout) }
            }
        }
        assert_eq!(Io::init(&mut Nothing).unwrap_err(), Error::Timeout);
    }

    #[test]
    fn bytes_and_blocks() {
        let mut m = Model::new();
        let mut io = Io::init(&mut m).unwrap();
        io.write_byte(&mut m, 1, 0x100, 0x5a).unwrap();
        assert_eq!(io.read_byte(&mut m, 1, 0x100).unwrap(), 0x5a);
        // A function the card does not have: FUNCTION_NUMBER in R5.
        assert_eq!(io.read_byte(&mut m, 3, 0).unwrap_err(), Error::Card);
        // Byte mode without a block size, incrementing.
        let mut out: Vec<u8> = (0..300u32).map(|i| i as u8).collect();
        io.transfer(&mut m, 1, 0x200, true, &mut out, true).unwrap();
        assert_eq!(&m.f1[0x200..0x200 + 300], &out[..]);
        let mut back = vec![0u8; 300];
        io.transfer(&mut m, 1, 0x200, true, &mut back, false).unwrap();
        assert_eq!(back, out);
        assert_eq!(m.log.last().unwrap().1 & 0x1ff, 300);
        // A whole 512 bytes in byte mode is written as a count of 0; a
        // FIFO's address does not move.
        let mut fifo: Vec<u8> = (0..512u32).map(|i| (i * 3) as u8).collect();
        io.transfer(&mut m, 1, 0, false, &mut fifo, true).unwrap();
        let arg = m.log.last().unwrap().1;
        assert_eq!((arg & (1 << 26), arg & (1 << 27), arg & 0x1ff), (0, 0, 0));
        assert_eq!(m.fifo, fifo);
        let mut got = vec![0u8; 512];
        io.transfer(&mut m, 1, 0, false, &mut got, false).unwrap();
        assert_eq!(got, fifo);
        // More than 512 bytes need block mode, so a block size.
        let mut big = vec![1u8; 1024];
        assert_eq!(io.transfer(&mut m, 1, 0x400, true, &mut big, true).unwrap_err(), Error::Card);
        io.set_block_size(&mut m, 1, 256).unwrap();
        assert_eq!((m.f0[0x110], m.f0[0x111]), (0, 1));
        io.transfer(&mut m, 1, 0x400, true, &mut big, true).unwrap();
        let arg = m.log.last().unwrap().1;
        assert!(arg & (1 << 27) != 0, "block mode");
        assert_eq!(arg & 0x1ff, 4);
        assert!(m.f1[0x400..0x800].iter().all(|&b| b == 1));
        // With a block size, a whole number of blocks goes in block mode.
        m.fifo.clear();
        let mut fifo = vec![7u8; 512];
        io.transfer(&mut m, 1, 0, false, &mut fifo, true).unwrap();
        let arg = m.log.last().unwrap().1;
        assert_eq!((arg & (1 << 26), arg & (1 << 27), arg & 0x1ff), (0, 1 << 27, 2));
        assert_eq!(m.fifo, fifo);
    }

    #[test]
    fn arguments() {
        assert_eq!(cmd::rw_direct(true, 1, 0x1000e, 0x08, false), 0x9200_1c08);
        assert_eq!(cmd::rw_direct(false, 0, 0x07, 0, false), 0x0000_0e00);
        assert_eq!(cmd::rw_extended(false, 2, 0x8000, false, true, 3), 0x2900_0003);
        assert_eq!(cmd::rw_extended(true, 1, 0x1, true, false, 512), 0x9400_0200);
        let r4 = R4::from(0xa030_0000);
        assert_eq!(r4, R4 { ready: true, functions: 2, memory_present: false, ocr: 0x0030_0000 });
        assert_eq!(R5::checked([0x1000_0042, 0, 0, 0]).unwrap(), R5 { flags: 0x00, data: 0x42 });
        assert_eq!(R5::checked([0x0800, 0, 0, 0]).unwrap_err(), Error::Card);
    }
}
