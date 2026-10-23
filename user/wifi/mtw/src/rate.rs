//! Which rate a data frame to the access point goes at: the rates both
//! ends have, slowest first, and AMRR choosing among them from what the
//! last second's frames needed (OpenBSD's ieee80211_amrr.c and
//! mtw_newassoc, mtw_calibrate_cb; notices in LICENSE). An 802.11n link's
//! rates are the MCS of one stream at 20 MHz, a legacy one's the 802.11b
//! and g rates.

use alloc::vec::Vec;

use crate::reg::{RATES, RIDX_CCK1};

/// A rate: legacy (an index of `reg::RATES`) or HT (an MCS, 0 to 7).
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum TxRate {
    Legacy(usize),
    Ht(u8),
}

impl TxRate {
    /// The legacy rate an ACK to a frame at this rate comes back at: a
    /// legacy rate's control rate, an MCS's non-HT reference rate down to
    /// a mandatory OFDM one (6, 12 or 24 Mb/s; IEEE 802.11-2020 10.6.6).
    pub fn ctl_ridx(self) -> usize {
        match self {
            TxRate::Legacy(r) => RATES[r].ctl_ridx,
            TxRate::Ht(0) => 4,
            TxRate::Ht(1 | 2) => 6,
            TxRate::Ht(_) => 8,
        }
    }
}

/// The rates of a link, slowest first.
#[derive(Debug, PartialEq, Eq, Clone)]
pub struct RateSet(pub Vec<TxRate>);

impl RateSet {
    /// The rates of a link with an access point that has legacy rates
    /// `rates` (as its Supported Rates elements say them: 500 kb/s units,
    /// the basic bit set or not), and over 802.11n when `ht_mcs` is its
    /// first MCS bitmap byte (the MCS 0 to 7 it receives).
    pub fn new(rates: &[u8], ht_mcs: Option<u8>) -> Self {
        if let Some(mcs) = ht_mcs.filter(|&m| m != 0) {
            return RateSet((0..8).filter(|m| mcs & 1 << m != 0).map(TxRate::Ht).collect());
        }
        let mut set: Vec<usize> =
            (0..RATES.len()).filter(|&r| rates.iter().any(|&v| v & 0x7f == RATES[r].rate)).collect();
        if set.is_empty() {
            set.push(RIDX_CCK1);
        }
        set.sort_by_key(|&r| RATES[r].rate);
        RateSet(set.into_iter().map(TxRate::Legacy).collect())
    }
}

/// AMRR's thresholds: successes in a row before going up (mtw's).
const MIN_SUCCESS_THRESHOLD: u32 = 1;
const MAX_SUCCESS_THRESHOLD: u32 = 10;

/// AMRR (Lacage, Manshaei, Turletti: "IEEE 802.11 Rate Adaptation: A
/// Practical Approach") for one link: up a rate after enough good
/// periods, down at a bad one, waiting longer to try again each time an
/// increase does not hold.
#[derive(Debug, Clone)]
pub struct Amrr {
    pub rates: RateSet,
    /// The index in `rates` frames go at.
    pub txrate: usize,
    success: u32,
    recovery: bool,
    success_threshold: u32,
    txcnt: u32,
    retrycnt: u32,
}

impl Amrr {
    /// A link that starts at its slowest rate (mtw_newassoc).
    pub fn new(rates: RateSet) -> Self {
        Amrr {
            rates,
            txrate: 0,
            success: 0,
            recovery: false,
            success_threshold: MIN_SUCCESS_THRESHOLD,
            txcnt: 0,
            retrycnt: 0,
        }
    }

    pub fn rate(&self) -> TxRate {
        self.rates.0[self.txrate]
    }

    /// The last period's frames: those sent (with their retries) and the
    /// retries and failures among them (mtw_calibrate_cb's counts), then a
    /// rate chosen (ieee80211_amrr_choose).
    pub fn update(&mut self, txcnt: u32, retrycnt: u32) {
        self.txcnt += txcnt;
        self.retrycnt += retrycnt;
        let enough = self.txcnt > 10;
        let max = self.txrate + 1 == self.rates.0.len();
        let mut changed = false;
        if self.retrycnt < self.txcnt / 10 && enough {
            self.success += 1;
            if self.success >= self.success_threshold && !max {
                self.recovery = true;
                self.success = 0;
                self.txrate += 1;
                changed = true;
            } else {
                self.recovery = false;
            }
        } else if self.retrycnt > self.txcnt / 3 {
            self.success = 0;
            if self.txrate > 0 {
                self.success_threshold = if self.recovery {
                    (self.success_threshold * 2).min(MAX_SUCCESS_THRESHOLD)
                } else {
                    MIN_SUCCESS_THRESHOLD
                };
                self.txrate -= 1;
                changed = true;
            }
            self.recovery = false;
        }
        if enough || changed {
            self.txcnt = 0;
            self.retrycnt = 0;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sets() {
        // An 802.11g access point's rates, the basic ones marked: slowest
        // first, CCK and OFDM interleaved by speed.
        let g = RateSet::new(&[0x82, 0x84, 0x8b, 0x96, 0x0c, 0x12, 0x18, 0x24, 0x30, 0x48, 0x60, 0x6c], None);
        let speeds: Vec<u8> = g.0.iter().map(|r| match r {
            TxRate::Legacy(i) => RATES[*i].rate,
            TxRate::Ht(_) => 0,
        }).collect();
        assert_eq!(speeds, [2, 4, 11, 12, 18, 22, 24, 36, 48, 72, 96, 108]);
        let n = RateSet::new(&[0x82], Some(0xff));
        assert_eq!(n.0, (0..8).map(TxRate::Ht).collect::<Vec<_>>());
        assert_eq!(RateSet::new(&[], None).0, [TxRate::Legacy(RIDX_CCK1)]);
        assert_eq!((TxRate::Ht(0).ctl_ridx(), TxRate::Ht(7).ctl_ridx()), (4, 8));
    }

    #[test]
    fn climbs_and_falls() {
        let mut a = Amrr::new(RateSet::new(&[0x0c, 0x18, 0x30], None));
        // Clean periods: up a rate each (the threshold is 1), to the top.
        for want in [1, 2, 2] {
            a.update(20, 0);
            assert_eq!(a.txrate, want);
        }
        // A bad period right after an increase: down, and the next
        // increase waits two good periods.
        let mut b = Amrr::new(RateSet::new(&[0x0c, 0x18, 0x30], None));
        b.update(20, 0);
        b.update(20, 15);
        assert_eq!(b.txrate, 0);
        b.update(20, 0);
        assert_eq!(b.txrate, 0);
        b.update(20, 0);
        assert_eq!(b.txrate, 1);
        // Too few frames say nothing.
        let mut c = Amrr::new(RateSet::new(&[0x0c, 0x18], None));
        c.update(5, 0);
        assert_eq!(c.txrate, 0);
    }
}
