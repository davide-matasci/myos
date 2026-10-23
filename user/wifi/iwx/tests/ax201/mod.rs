//! A model of the AX201 for the driver's tests (`tests/ax201_model.rs`):
//! the device the `iwx` crate drives, its registers, rings and interrupt
//! (`device.rs`), the firmware behind them (`firmware.rs`: its self load
//! of the synthetic file `iwx::sim`, the bring-up, the scan, joining a
//! network, the station's frames) and the access points it hears
//! (`air.rs`). Every command and descriptor the driver gives it is checked
//! against the firmware's structure for it, as OpenBSD's if_iwxreg.h
//! describes them, and what a real device would not take stops it with
//! the reason. myos's own code, not taken from OpenBSD or Intel.

pub mod air;
pub mod device;
pub mod firmware;
