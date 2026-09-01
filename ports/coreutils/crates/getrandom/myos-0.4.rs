//! myos: non-cryptographic PRNG until the kernel exposes real entropy.
use crate::Error;
use core::mem::MaybeUninit;
use core::sync::atomic::{AtomicU64, Ordering};

static SEED: AtomicU64 = AtomicU64::new(0x243F_6A88_85A3_08D3);

fn next_u64() -> u64 {
    let mut x = SEED.load(Ordering::Relaxed);
    x ^= x << 13;
    x ^= x >> 7;
    x ^= x << 17;
    SEED.store(x, Ordering::Relaxed);
    x
}

pub use crate::util::{inner_u32, inner_u64};

pub fn fill_inner(dest: &mut [MaybeUninit<u8>]) -> Result<(), Error> {
    let mut off = 0usize;
    while off < dest.len() {
        let word = next_u64().to_le_bytes();
        let take = (dest.len() - off).min(word.len());
        for (d, &b) in dest[off..off + take].iter_mut().zip(&word[..take]) {
            d.write(b);
        }
        off += take;
    }
    Ok(())
}
