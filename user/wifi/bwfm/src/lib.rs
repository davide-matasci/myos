//! The logic of the wifi daemon's driver for Broadcom's and Cypress's
//! FullMAC Wi-Fi chips over SDIO (the Raspberry Pi's CYW43455 and
//! CYW43438, `chip`), ported from OpenBSD's bwfm(4): the chip's cores
//! behind the SDIO function 1 backplane window (`chip`), the SDIO bus side
//! (`bus`: clocks, the firmware and NVRAM into the chip's RAM, SDPCM
//! frames in and out), the BCDC control protocol (`bcdc`: the firmware's
//! commands and variables), its events (`event`: scan results, the link),
//! and the FullMAC driver over them (`dev`: bring-up, scan, join, leave,
//! Ethernet frames). The firmware scans, authenticates, associates and
//! runs WPA2-PSK itself; the host trades Ethernet frames and commands with
//! it. The daemon gives the crate the card (`Sdio`: CMD52 and CMD53 on its
//! functions; `/dev/sdio` on myos) and the firmware files, and asks it to
//! service the card when its interrupt fires, so all of it builds and is
//! tested on the host against a model of the chip.
//!
//! Copyright and licence: LICENSE, the notices of OpenBSD's bwfm.c,
//! bwfmreg.h, bwfmvar.h, if_bwfm_sdio.c and if_bwfm_sdio.h (ISC). The
//! code derived from them stays under those terms.

#![no_std]

extern crate alloc;

pub mod bcdc;
pub mod bus;
pub mod chip;
pub mod dev;
pub mod event;
pub mod fw;
pub mod reg;
pub mod sdio;

/// What went wrong with the card, the chip or the firmware.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    /// A transfer on the SDIO bus failed.
    Sdio,
    /// The chip is not what the driver expects (what).
    Chip(&'static str),
    /// The firmware files, or their download.
    Firmware(&'static str),
    /// A frame or message the firmware sent is malformed (what).
    Protocol(&'static str),
    /// A wait ran out (for what).
    Timeout(&'static str),
    /// The firmware refused a command: its status.
    Status(i32),
}

impl From<sdio::SdioError> for Error {
    fn from(_: sdio::SdioError) -> Self {
        Error::Sdio
    }
}
