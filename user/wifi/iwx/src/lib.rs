//! The logic of the wifi daemon's driver for Intel's Wi-Fi 6 radios (the
//! AX201 of issue #495), ported from OpenBSD's iwx(4): the firmware file,
//! the device's registers and bring-up, its command and receive rings, the
//! commands and notifications up to a scan. The daemon gives it the device
//! (`Bus`: the registers, DMA memory, the interrupt; `/dev/pci` on myos),
//! so all of it builds and its encodings are tested on the host.
//!
//! Copyright and licence: LICENSE, the notices of OpenBSD's if_iwx.c,
//! if_iwxreg.h and if_iwxvar.h (ISC; Intel's part dual BSD/GPLv2, taken
//! under the BSD licence). The code derived from them stays under those
//! terms.

#![no_std]

extern crate alloc;

pub mod fw;
pub mod hw;
pub mod reg;
pub mod ring;
pub mod msg;
pub mod dev;
