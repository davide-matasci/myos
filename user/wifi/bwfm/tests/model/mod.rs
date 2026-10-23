//! A model of a Broadcom FullMAC chip on SDIO, for the driver's tests: the
//! card's functions and registers, the chip's cores behind the backplane
//! window, its RAM, and a firmware that, once started, answers the
//! driver's commands, scans a made-up air, joins its networks and carries
//! Ethernet frames.

pub mod device;
