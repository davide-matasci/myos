//! The logic of the wifi daemon's driver for MediaTek's USB Wi-Fi radios
//! (`chip`): the MT7601U's, ported from OpenBSD's mtw(4), and the
//! MT7612U's (`mt76x2`), from Linux's mt76x2u. The firmware files, the
//! registers behind the device's vendor requests, the MAC, the baseband
//! and RF bring-up, the channels, and the descriptors the frames go out
//! and come in with. A SoftMAC: the daemon scans, joins and encrypts, the
//! radio sends and hears frames. The daemon gives it the device (`Usb`:
//! control transfers, the bulk OUT endpoints and the MCU's answers;
//! `/dev/usb` on myos) and hands it what the frames' bulk IN endpoint
//! brought (`rx`, `mt76x2::frame`), so all of it builds and its encodings
//! are tested on the host.
//!
//! Copyright and licence: LICENSE, the notices of OpenBSD's if_mtw.c,
//! if_mtwvar.h, mtwreg.h and ieee80211_amrr.c (ISC), and of Linux's mt76
//! files (BSD-3-Clause-Clear). The code derived from them stays under
//! those terms.

#![no_std]

extern crate alloc;

pub mod chip;
pub mod reg;
pub mod fw;
pub mod usb;
pub mod rate;
pub mod rx;
pub mod tx;
pub mod dev;
pub mod mt76x2;
