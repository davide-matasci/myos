//! The SDIO side of the chip (ported from if_bwfm_sdio.c): function 0's
//! and 1's registers, the backplane through function 1's window, the
//! chip's clocks, the firmware and NVRAM into its RAM and the CPU started
//! on them, then SDPCM frames on function 2: control messages, events and
//! data in and out, with the firmware's credits.

use alloc::vec;
use alloc::vec::Vec;

use crate::Error;
use crate::chip::{Backplane, Chip};
use crate::reg::*;
use crate::sdio::Sdio;

/// What the card is told the clock should be.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Clk {
    None,
    SdOnly,
    Pending,
    Avail,
}

/// Function 1's and 2's block sizes.
pub const F1_BLOCK: u16 = 64;
pub const F2_BLOCK: u16 = 512;
/// The most a frame may be, in or out.
pub const FRAME_MAX: usize = 64 * 1024;

/// A frame the card sent, by channel.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Channel {
    Control,
    Event,
    Data,
}

pub struct Bus<S: Sdio> {
    pub sdio: S,
    pub chip: Chip,
    /// The backplane window set (`u32::MAX`: none yet).
    bar0: u32,
    clkstate: Clk,
    alp_only: bool,
    sr_enabled: bool,
    /// The SDIO device core's and chipcommon's registers.
    sdiod: u32,
    cc: u32,
    /// The next frame's sequence number and the last the firmware allows.
    tx_seq: u8,
    tx_max_seq: u8,
    /// The firmware's console in RAM, when the shared structure says.
    pub console_addr: u32,
    bounce: Vec<u8>,
}

impl<S: Sdio> Backplane for Bus<S> {
    fn read32(&mut self, addr: u32) -> Result<u32, Error> {
        self.read_4(addr)
    }
    fn write32(&mut self, addr: u32, value: u32) -> Result<(), Error> {
        self.write_4(addr, value)
    }
    fn delay_us(&mut self, us: u32) {
        self.sdio.delay_us(us);
    }
}

impl<S: Sdio> Bus<S> {
    /// Attach to the card: its functions' block sizes, function 1 enabled,
    /// the chip's cores read and its CPU held, ready for the firmware.
    pub fn attach(sdio: S) -> Result<Bus<S>, Error> {
        let mut bus = Bus {
            sdio,
            chip: Chip::empty(),
            bar0: u32::MAX,
            clkstate: Clk::None,
            alp_only: false,
            sr_enabled: false,
            sdiod: 0,
            cc: 0,
            tx_seq: 0xff,
            tx_max_seq: 0,
            console_addr: 0,
            bounce: vec![0; FRAME_MAX],
        };
        bus.sdio.set_block_size(1, F1_BLOCK)?;
        bus.sdio.set_block_size(2, F2_BLOCK)?;
        bus.function_enable(1, true)?;
        // Force the PLL off.
        bus.write_1(FUNC1_CHIPCLKCSR, CHIPCLKCSR_FORCE_HW_CLKREQ_OFF | CHIPCLKCSR_ALP_AVAIL_REQ)?;
        bus.prepare()?;
        let chip = Chip::attach(&mut bus)?;
        bus.cc = chip.core(CORE_CHIPCOMMON).ok_or(Error::Chip("no chipcommon core"))?.base;
        let sdiod = chip.core(CORE_SDIO_DEV).ok_or(Error::Chip("no SDIO device core"))?;
        bus.sdiod = sdiod.base;
        if sdiod.rev >= 12 {
            let v = bus.read_1(FUNC1_SLEEPCSR)?;
            if v & SLEEPCSR_KSO == 0 {
                bus.write_1(FUNC1_SLEEPCSR, v | SLEEPCSR_KSO)?;
            }
        }
        let v = bus.read_1(CCCR_CARDCTRL)?;
        bus.write_1(CCCR_CARDCTRL, v | CCCR_CARDCTRL_WLANRESET)?;
        let pmu = chip.pmu();
        let v = bus.read_4(pmu.base + CHIP_REG_PMUCONTROL)?;
        bus.write_4(pmu.base + CHIP_REG_PMUCONTROL, v | (CHIP_REG_PMUCONTROL_RES_RELOAD << CHIP_REG_PMUCONTROL_RES_SHIFT))?;
        bus.function_enable(2, false)?;
        bus.write_1(FUNC1_CHIPCLKCSR, 0)?;
        bus.clkstate = Clk::SdOnly;
        bus.chip = chip;
        Ok(bus)
    }

    /// Function `f` on or off (the CCCR's I/O enable), waiting for it to
    /// be ready when on.
    fn function_enable(&mut self, f: u8, on: bool) -> Result<(), Error> {
        let bit = 1u8 << f;
        let v = self.sdio.read_1(0, 0x02)?;
        self.sdio.write_1(0, 0x02, if on { v | bit } else { v & !bit })?;
        if !on {
            return Ok(());
        }
        let start = self.sdio.now_ms();
        loop {
            if self.sdio.read_1(0, 0x03)? & bit != 0 {
                return Ok(());
            }
            if self.sdio.now_ms() - start > 5000 {
                return Err(Error::Timeout("function ready"));
            }
            self.sdio.delay_us(1000);
        }
    }

    /// Function 0's and 1's registers by address: the CCCR and FBRs below
    /// `0x800`, function 1's own and the backplane window above.
    pub fn read_1(&mut self, addr: u32) -> Result<u8, Error> {
        let f = if addr & !0x7ff == 0 { 0 } else { 1 };
        Ok(self.sdio.read_1(f, addr)?)
    }

    pub fn write_1(&mut self, addr: u32, value: u8) -> Result<(), Error> {
        let f = if addr & !0x7ff == 0 { 0 } else { 1 };
        Ok(self.sdio.write_1(f, addr, value)?)
    }

    /// The backplane window onto the 32 KiB page of `bar0`.
    fn backplane(&mut self, bar0: u32) -> Result<(), Error> {
        if self.bar0 == bar0 {
            return Ok(());
        }
        self.sdio.write_1(1, FUNC1_SBADDRLOW, ((bar0 >> 8) & 0x80) as u8)?;
        self.sdio.write_1(1, FUNC1_SBADDRMID, (bar0 >> 16) as u8)?;
        self.sdio.write_1(1, FUNC1_SBADDRHIGH, (bar0 >> 24) as u8)?;
        self.bar0 = bar0;
        Ok(())
    }

    /// A 32-bit backplane register.
    pub fn read_4(&mut self, addr: u32) -> Result<u32, Error> {
        self.backplane(addr & !SB_OFT_ADDR_MASK)?;
        let mut v = [0u8; 4];
        self.sdio.read(1, (addr & SB_OFT_ADDR_MASK) | SB_ACCESS_2_4B_FLAG, &mut v, true)?;
        Ok(u32::from_le_bytes(v))
    }

    pub fn write_4(&mut self, addr: u32, value: u32) -> Result<(), Error> {
        self.backplane(addr & !SB_OFT_ADDR_MASK)?;
        Ok(self.sdio.write(1, (addr & SB_OFT_ADDR_MASK) | SB_ACCESS_2_4B_FLAG, &value.to_le_bytes(), true)?)
    }

    /// The SDIO device core's registers.
    pub fn dev_read(&mut self, reg: u32) -> Result<u32, Error> {
        self.read_4(self.sdiod + reg)
    }

    pub fn dev_write(&mut self, reg: u32, value: u32) -> Result<(), Error> {
        self.write_4(self.sdiod + reg, value)
    }

    /// `data` into (`write`) or from the chip's RAM at `addr`, a window
    /// page at a time, whole words.
    pub fn ram_write(&mut self, addr: u32, data: &[u8]) -> Result<(), Error> {
        let mut off = 0;
        while off < data.len() {
            let sbaddr = addr + off as u32;
            self.backplane(sbaddr & !SB_OFT_ADDR_MASK)?;
            let sdaddr = sbaddr & SB_OFT_ADDR_MASK;
            let size = (data.len() - off).min((SB_OFT_ADDR_PAGE - sdaddr) as usize);
            let padded = size.next_multiple_of(4);
            self.bounce[..size].copy_from_slice(&data[off..off + size]);
            self.bounce[size..padded].fill(0);
            let bounce = core::mem::take(&mut self.bounce);
            let r = self.sdio.write(1, sdaddr | SB_ACCESS_2_4B_FLAG, &bounce[..padded], true);
            self.bounce = bounce;
            r?;
            off += size;
        }
        Ok(())
    }

    pub fn ram_read(&mut self, addr: u32, data: &mut [u8]) -> Result<(), Error> {
        let mut off = 0;
        while off < data.len() {
            let sbaddr = addr + off as u32;
            self.backplane(sbaddr & !SB_OFT_ADDR_MASK)?;
            let sdaddr = sbaddr & SB_OFT_ADDR_MASK;
            let size = (data.len() - off).min((SB_OFT_ADDR_PAGE - sdaddr) as usize);
            let padded = size.next_multiple_of(4);
            let mut bounce = core::mem::take(&mut self.bounce);
            let r = self.sdio.read(1, sdaddr | SB_ACCESS_2_4B_FLAG, &mut bounce[..padded], true);
            data[off..off + size].copy_from_slice(&bounce[..size]);
            self.bounce = bounce;
            r?;
            off += size;
        }
        Ok(())
    }

    /// A frame on function 2 (at chipcommon's address: fixed, a FIFO).
    fn frame_write(&mut self, data: &[u8]) -> Result<(), Error> {
        let cc = self.cc;
        self.backplane(cc & !SB_OFT_ADDR_MASK)?;
        Ok(self.sdio.write(2, (cc & SB_OFT_ADDR_MASK) | SB_ACCESS_2_4B_FLAG, data, true)?)
    }

    fn frame_read(&mut self, data: &mut [u8]) -> Result<(), Error> {
        let cc = self.cc;
        self.backplane(cc & !SB_OFT_ADDR_MASK)?;
        Ok(self.sdio.read(2, (cc & SB_OFT_ADDR_MASK) | SB_ACCESS_2_4B_FLAG, data, false)?)
    }

    /// The backplane's clock before the chip is touched (the bus core's
    /// `prepare`): ALP requested and waited for, then forced.
    fn prepare(&mut self) -> Result<(), Error> {
        let clkset = CHIPCLKCSR_ALP_AVAIL_REQ | CHIPCLKCSR_FORCE_HW_CLKREQ_OFF;
        self.write_1(FUNC1_CHIPCLKCSR, clkset)?;
        let v = self.read_1(FUNC1_CHIPCLKCSR)?;
        if v & !CHIPCLKCSR_AVBITS != clkset {
            return Err(Error::Chip("clock request not taken"));
        }
        let mut i = 1000;
        while i > 0 && self.read_1(FUNC1_CHIPCLKCSR)? & CHIPCLKCSR_AVBITS == 0 {
            i -= 1;
        }
        if i == 0 {
            return Err(Error::Timeout("ALP clock"));
        }
        self.write_1(FUNC1_CHIPCLKCSR, CHIPCLKCSR_FORCE_HW_CLKREQ_OFF | CHIPCLKCSR_FORCE_ALP)?;
        self.sdio.delay_us(65);
        self.write_1(FUNC1_SDIOPULLUP, 0)?;
        Ok(())
    }

    /// The clock the host asks of the chip: the SD clock only, or the
    /// backplane's (HT, or ALP while the firmware loads) as well.
    fn clkctl(&mut self, new: Clk, pendok: bool) -> Result<(), Error> {
        if self.clkstate == new {
            return Ok(());
        }
        match new {
            Clk::Avail => {
                if self.clkstate == Clk::None {
                    self.clkstate = Clk::SdOnly;
                }
                self.htclk(true, pendok)
            }
            Clk::SdOnly => {
                if self.clkstate == Clk::None {
                    self.clkstate = Clk::SdOnly;
                    Ok(())
                } else if self.clkstate == Clk::Avail {
                    self.htclk(false, false)
                } else {
                    Ok(())
                }
            }
            Clk::None => {
                if self.clkstate == Clk::Avail {
                    self.htclk(false, false)?;
                }
                self.clkstate = Clk::None;
                Ok(())
            }
            Clk::Pending => Ok(()),
        }
    }

    fn htclk(&mut self, on: bool, pendok: bool) -> Result<(), Error> {
        if self.sr_enabled {
            self.clkstate = if on { Clk::Avail } else { Clk::SdOnly };
            return Ok(());
        }
        if !on {
            if self.clkstate == Clk::Pending {
                let v = self.read_1(SDIO_DEVICE_CTL)?;
                self.write_1(SDIO_DEVICE_CTL, v & !SDIO_DEVICE_CTL_CA_INT_ONLY)?;
            }
            self.clkstate = Clk::SdOnly;
            return self.write_1(FUNC1_CHIPCLKCSR, 0);
        }
        let req = if self.alp_only { CHIPCLKCSR_ALP_AVAIL_REQ } else { CHIPCLKCSR_HT_AVAIL_REQ };
        self.write_1(FUNC1_CHIPCLKCSR, req)?;
        let mut csr = self.read_1(FUNC1_CHIPCLKCSR)?;
        if !clock_available(csr, self.alp_only) && pendok {
            let v = self.read_1(SDIO_DEVICE_CTL)?;
            self.write_1(SDIO_DEVICE_CTL, v | SDIO_DEVICE_CTL_CA_INT_ONLY)?;
            self.clkstate = Clk::Pending;
            return Ok(());
        } else if self.clkstate == Clk::Pending {
            let v = self.read_1(SDIO_DEVICE_CTL)?;
            self.write_1(SDIO_DEVICE_CTL, v & !SDIO_DEVICE_CTL_CA_INT_ONLY)?;
        }
        let mut i = 0;
        while i < 5000 && !clock_available(csr, self.alp_only) {
            csr = self.read_1(FUNC1_CHIPCLKCSR)?;
            self.sdio.delay_us(1000);
            i += 1;
        }
        if !clock_available(csr, self.alp_only) {
            return Err(Error::Timeout("HT clock"));
        }
        self.clkstate = Clk::Avail;
        Ok(())
    }

    /// The firmware and the NVRAM into RAM, each read back and compared,
    /// then the CPU started on the firmware's reset vector.
    pub fn load_firmware(&mut self, firmware: &[u8], nvram: &[u8]) -> Result<(), Error> {
        if firmware.len() + nvram.len() > self.chip.ramsize as usize {
            return Err(Error::Firmware("firmware and NVRAM do not fit the RAM"));
        }
        self.alp_only = true;
        let r = self.load_firmware_clocked(firmware, nvram);
        self.alp_only = false;
        self.clkctl(Clk::SdOnly, false)?;
        r
    }

    fn load_firmware_clocked(&mut self, firmware: &[u8], nvram: &[u8]) -> Result<(), Error> {
        self.clkctl(Clk::Avail, false)?;
        let rambase = self.chip.rambase;
        self.ram_write(rambase, firmware)?;
        let mut verify = vec![0u8; firmware.len()];
        self.ram_read(rambase, &mut verify)?;
        if verify != firmware {
            return Err(Error::Firmware("firmware verification failed"));
        }
        let nvaddr = rambase + self.chip.ramsize - nvram.len() as u32;
        self.ram_write(nvaddr, nvram)?;
        let mut verify = vec![0u8; nvram.len()];
        self.ram_read(nvaddr, &mut verify)?;
        if verify != nvram {
            return Err(Error::Firmware("NVRAM verification failed"));
        }
        let rstvec = crate::fw::reset_vector(firmware);
        let chip = core::mem::replace(&mut self.chip, Chip::empty());
        let r = chip.set_active(self, rstvec, |bus, rstvec| {
            bus.dev_write(SDPCMD_INTSTATUS, 0xffff_ffff)?;
            if rstvec != 0 {
                bus.ram_write(0, &rstvec.to_le_bytes())?;
            }
            Ok(())
        });
        self.chip = chip;
        r
    }

    /// The firmware running: the HT clock, the protocol version told, function
    /// 2 up, the device core's interrupts to the host, the card's interrupt
    /// line enabled.
    pub fn start(&mut self) -> Result<(), Error> {
        self.clkctl(Clk::Avail, false)?;
        if self.clkstate != Clk::Avail {
            return Err(Error::Timeout("HT clock"));
        }
        let csr = self.read_1(FUNC1_CHIPCLKCSR)?;
        self.write_1(FUNC1_CHIPCLKCSR, csr | CHIPCLKCSR_FORCE_HT)?;
        self.dev_write(SDPCMD_TOSBMAILBOXDATA, SDPCM_PROT_VERSION << SDPCM_PROT_VERSION_SHIFT)?;
        self.function_enable(2, true)?;
        self.dev_write(SDPCMD_HOSTINTMASK, SDPCMD_INTSTATUS_HMB_SW_MASK | SDPCMD_INTSTATUS_CHIPACTIVE)?;
        self.write_1(SDIO_WATERMARK, 8)?;
        let chip = core::mem::replace(&mut self.chip, Chip::empty());
        let sr = chip.sr_capable(self);
        self.chip = chip;
        if sr? {
            let v = self.read_1(FUNC1_WAKEUPCTRL)?;
            self.write_1(FUNC1_WAKEUPCTRL, v | WAKEUPCTRL_HTWAIT)?;
            self.write_1(CCCR_CARDCAP, CCCR_CARDCAP_CMD14_SUPPORT | CCCR_CARDCAP_CMD14_EXT)?;
            self.write_1(FUNC1_CHIPCLKCSR, CHIPCLKCSR_FORCE_HT)?;
            self.sr_enabled = true;
        } else {
            self.write_1(FUNC1_CHIPCLKCSR, csr)?;
        }
        // The card's interrupt: function 1's, and the master enable.
        self.sdio.write_1(0, 0x04, 0x03)?;
        Ok(())
    }

    /// Whether the firmware has credit for another frame.
    pub fn tx_ok(&self) -> bool {
        let d = self.tx_max_seq.wrapping_sub(self.tx_seq);
        d != 0 && d & 0x80 == 0
    }

    /// `payload` on `channel` (with a BCDC header `bcdc` first on the data
    /// channel): one SDPCM frame, padded to a word or, past 512 bytes, to
    /// a block.
    pub fn tx_frame(&mut self, channel: Channel, bcdc: Option<[u8; BCDC_HDR_LEN]>, payload: &[u8]) -> Result<(), Error> {
        // The data offset is the SDPCM headers': the BCDC header is the
        // payload's first word.
        let len = SDPCM_HDR_LEN + bcdc.map_or(0, |_| BCDC_HDR_LEN) + payload.len();
        if len > FRAME_MAX {
            return Err(Error::Protocol("frame too long"));
        }
        let roundto = if len > 512 && len % 512 != 0 { 512 } else { 4 };
        let total = len.next_multiple_of(roundto);
        let mut f = Vec::with_capacity(total);
        f.extend_from_slice(&(len as u16).to_le_bytes());
        f.extend_from_slice(&(!(len as u16)).to_le_bytes());
        let seq = self.tx_seq;
        self.tx_seq = self.tx_seq.wrapping_add(1);
        let chan = match channel {
            Channel::Control => SDPCM_CHANNEL_CONTROL,
            Channel::Event => SDPCM_CHANNEL_EVENT,
            Channel::Data => SDPCM_CHANNEL_DATA,
        };
        f.extend_from_slice(&[seq, chan, 0, SDPCM_HDR_LEN as u8, 0, 0, 0, 0]);
        if let Some(b) = bcdc {
            f.extend_from_slice(&b);
        }
        f.extend_from_slice(payload);
        f.resize(total, 0);
        self.frame_write(&f)
    }

    /// Serve the card: its device core's interrupts (the firmware's
    /// mailbox), then every frame waiting, each to `sink` with its channel
    /// and payload (past the data offset).
    pub fn service(&mut self, sink: &mut dyn FnMut(Channel, &[u8])) -> Result<(), Error> {
        if !self.sr_enabled && self.clkstate == Clk::Pending {
            let csr = self.read_1(FUNC1_CHIPCLKCSR)?;
            if csr & CHIPCLKCSR_AVBITS == CHIPCLKCSR_AVBITS {
                let v = self.read_1(SDIO_DEVICE_CTL)?;
                self.write_1(SDIO_DEVICE_CTL, v & !SDIO_DEVICE_CTL_CA_INT_ONLY)?;
                self.clkstate = Clk::Avail;
            }
        }
        let intstat = self.dev_read(SDPCMD_INTSTATUS)? & (SDPCMD_INTSTATUS_HMB_SW_MASK | SDPCMD_INTSTATUS_CHIPACTIVE);
        if intstat != 0 {
            self.dev_write(SDPCMD_INTSTATUS, intstat)?;
        }
        if intstat & SDPCMD_INTSTATUS_HMB_HOST_INT != 0 {
            let hostint = self.dev_read(SDPCMD_TOHOSTMAILBOXDATA)?;
            self.dev_write(SDPCMD_TOSBMAILBOX, SDPCMD_TOSBMAILBOX_INT_ACK)?;
            if hostint & (SDPCMD_TOHOSTMAILBOXDATA_DEVREADY | SDPCMD_TOHOSTMAILBOXDATA_FWREADY) != 0 {
                self.read_shared()?;
            }
        }
        self.rx_frames(sink)
    }

    /// The firmware's shared structure at the end of RAM: where its console
    /// is.
    fn read_shared(&mut self) -> Result<(), Error> {
        let mut shaddr = self.chip.rambase + self.chip.ramsize - 4;
        if self.chip.rambase == 0 {
            let chip = core::mem::replace(&mut self.chip, Chip::empty());
            let sr = chip.sr_capable(self);
            self.chip = chip;
            if sr? {
                shaddr -= self.chip.srsize;
            }
        }
        let mut w = [0u8; 4];
        self.ram_read(shaddr, &mut w)?;
        let addr = u32::from_le_bytes(w);
        if addr == 0 || (!addr >> 16) & 0xffff == addr & 0xffff {
            return Ok(());
        }
        let mut sdpcm = [0u8; 24];
        self.ram_read(addr, &mut sdpcm)?;
        let c = &sdpcm[SDPCM_SHARED_CONSOLE_ADDR..];
        self.console_addr = u32::from_le_bytes([c[0], c[1], c[2], c[3]]);
        Ok(())
    }

    /// Every frame the card has: read (ahead, when the last said the next
    /// one's length) and handed to `sink`; a glom (superframe) unpacked.
    fn rx_frames(&mut self, sink: &mut dyn FnMut(Channel, &[u8])) -> Result<(), Error> {
        let mut nextlen = 0usize;
        loop {
            let mut buf = core::mem::take(&mut self.bounce);
            let r = if nextlen != 0 { self.frame_read(&mut buf[..nextlen]) } else { self.frame_read(&mut buf[..SDPCM_HDR_LEN]) };
            if let Err(e) = r {
                self.bounce = buf;
                return Err(e);
            }
            let frmlen = usize::from(u16::from_le_bytes([buf[0], buf[1]]));
            let cksum = u16::from_le_bytes([buf[2], buf[3]]);
            if frmlen == 0 && cksum == 0 {
                self.bounce = buf;
                return Ok(());
            }
            if (frmlen as u16 ^ cksum) != 0xffff || frmlen < SDPCM_HDR_LEN || (nextlen != 0 && frmlen > nextlen) {
                self.bounce = buf;
                return Err(Error::Protocol("frame header"));
            }
            let sw = [buf[4], buf[5], buf[6], buf[7], buf[8], buf[9]];
            self.tx_max_seq = sw[5];
            let flen = frmlen - SDPCM_HDR_LEN;
            if flen == 0 {
                nextlen = usize::from(sw[2]) << 4;
                self.bounce = buf;
                continue;
            }
            if nextlen == 0 {
                let padded = flen.next_multiple_of(4);
                if SDPCM_HDR_LEN + padded > buf.len() {
                    self.bounce = buf;
                    return Err(Error::Protocol("frame too long"));
                }
                if let Err(e) = self.frame_read(&mut buf[SDPCM_HDR_LEN..SDPCM_HDR_LEN + padded]) {
                    self.bounce = buf;
                    return Err(e);
                }
            }
            let dataoff = usize::from(sw[3]);
            if dataoff < SDPCM_HDR_LEN || dataoff > frmlen {
                self.bounce = buf;
                return Err(Error::Protocol("data offset"));
            }
            let data = &buf[dataoff..frmlen];
            nextlen = usize::from(sw[2]) << 4;
            match sw[1] & SDPCM_CHANNEL_MASK {
                SDPCM_CHANNEL_CONTROL => sink(Channel::Control, data),
                SDPCM_CHANNEL_EVENT => sink(Channel::Event, data),
                SDPCM_CHANNEL_DATA => sink(Channel::Data, data),
                SDPCM_CHANNEL_GLOM => {
                    // The sub-frames' lengths, then the sub-frames: the
                    // first with a superframe header to skip, each with
                    // its own.
                    let sublen: Vec<usize> = buf[SDPCM_HDR_LEN..frmlen].chunks_exact(2).map(|c| usize::from(u16::from_le_bytes([c[0], c[1]]))).collect();
                    self.bounce = buf;
                    let mut subs = Vec::with_capacity(sublen.len());
                    for (i, &n) in sublen.iter().enumerate() {
                        if n > FRAME_MAX {
                            return Err(Error::Protocol("glom sub-frame"));
                        }
                        let mut m = vec![0u8; n];
                        self.frame_read(&mut m)?;
                        if i == 0 && n >= SDPCM_HDR_LEN {
                            nextlen = usize::from(m[6]) << 4;
                            m.drain(..SDPCM_HDR_LEN);
                        }
                        subs.push(m);
                    }
                    for m in &subs {
                        if m.len() < SDPCM_HDR_LEN {
                            continue;
                        }
                        let l = usize::from(u16::from_le_bytes([m[0], m[1]]));
                        let c = u16::from_le_bytes([m[2], m[3]]);
                        if l == 0 || (l as u16 ^ c) != 0xffff || l < SDPCM_HDR_LEN || l > m.len() {
                            continue;
                        }
                        let off = usize::from(m[7]);
                        if off < SDPCM_HDR_LEN || off > l {
                            continue;
                        }
                        match m[5] & SDPCM_CHANNEL_MASK {
                            SDPCM_CHANNEL_EVENT => sink(Channel::Event, &m[off..l]),
                            SDPCM_CHANNEL_DATA => sink(Channel::Data, &m[off..l]),
                            _ => {}
                        }
                    }
                    continue;
                }
                _ => {}
            }
            self.bounce = buf;
        }
    }
}
