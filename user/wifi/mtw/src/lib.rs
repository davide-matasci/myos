//! The logic of the wifi daemon's driver for MediaTek's USB Wi-Fi radios
//! (the MT7601U, `chip`), ported from OpenBSD's mtw(4): the firmware
//! file, the registers behind the device's vendor requests, the MAC, the
//! baseband and RF bring-up, the channels, and the descriptors the frames
//! go out and come in with. A SoftMAC: the daemon scans, joins and
//! encrypts, the radio sends and hears frames. The daemon gives it the
//! device (`Usb`: control transfers and the bulk OUT endpoints;
//! `/dev/usb` on myos) and hands it what the bulk IN endpoint brought
//! (`rx`), so all of it builds and its encodings are tested on the host.
//!
//! Copyright and licence: LICENSE, the notices of OpenBSD's if_mtw.c,
//! if_mtwvar.h and mtwreg.h (ISC). The code derived from them stays under
//! those terms.

#![no_std]

extern crate alloc;

pub mod chip;
pub mod reg;
pub mod fw;
pub mod usb;
pub mod rx;
pub mod tx;
pub mod dev;
