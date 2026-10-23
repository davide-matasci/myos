//! A model of the MT7601U for the driver's tests (`tests/radio.rs`): the
//! device the `mtw` crate drives, its registers behind the vendor
//! requests, eFUSE ROM, RF and baseband windows, its MCU taking the
//! firmware and commands, and its bulk endpoints (`device.rs`), hearing
//! the access points of the iwx model's air (`air.rs`, shared). Every
//! request and descriptor the driver gives it is checked against the
//! layout OpenBSD's mtwreg.h describes, and what a real device would not
//! take stops it with the reason. myos's own code, not taken from OpenBSD
//! or MediaTek.

#[path = "../../../iwx/tests/model/air.rs"]
pub mod air;
pub mod device;
