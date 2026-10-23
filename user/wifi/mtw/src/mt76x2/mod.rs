//! The MT76x2U (the MT7612U), ported from Linux's mt76x2u driver and the
//! mt76 and mt76x02 code it shares (drivers/net/wireless/mediatek/mt76;
//! notices in LICENSE): its registers (`reg`), firmware files (`fw`),
//! frames on the bulk endpoints (`frame`) and the radio (`dev`). The same
//! SoftMAC as the MT7601U's `dev`, with two streams and 5 GHz; the MCU
//! answers its commands on the second bulk IN endpoint.

pub mod reg;
pub mod fw;
pub mod frame;
pub mod dev;
