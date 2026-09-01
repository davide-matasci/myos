//! Weak PRNG for myos bring-up (no hardware RNG yet).

use core::sync::atomic::{AtomicU64, Ordering};

use crate::ptr;

static SEED: AtomicU64 = AtomicU64::new(0);

pub fn fill_bytes(bytes: &mut [u8]) {
    let mut s = SEED.load(Ordering::Relaxed);
    if s == 0 {
        s = ptr::from_ref(&s).addr() as u64 ^ 0x9E37_79B9_7F4A_7C15;
    }
    for b in bytes.iter_mut() {
        s = s.wrapping_mul(6364136223846793005).wrapping_add(1);
        *b = (s >> 33) as u8;
    }
    SEED.store(s, Ordering::Relaxed);
}

pub fn hashmap_random_keys() -> (u64, u64) {
    let mut buf = [0u8; 16];
    fill_bytes(&mut buf);
    let k1 = u64::from_le_bytes(buf[..8].try_into().unwrap());
    let k2 = u64::from_le_bytes(buf[8..].try_into().unwrap());
    (k1, k2)
}
