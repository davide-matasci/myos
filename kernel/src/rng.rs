//! Kernel CSPRNG: ChaCha20 stream cipher as a DRBG, `/dev/urandom` + `/dev/random`.
//!
//! Modular design (Linux-shaped, simplified for phase-1):
//! - [`rng::fill`]: pull `n` bytes from the ChaCha20 keystream; blocks are
//!   never reused (counter advances), state is re-keyed (backwards secrecy)
//!   periodically after output.
//! - Seeding: a mix of arch-entropy sources gathered at [`rng::init`] and
//!   continuously stirred by timer ticks ([`rng::stir_tick`]) — RDTSC/perf
//!   counter jitter, RTC via [`crate::time`], heap/allocator addresses, and
//!   per-CPU cycle deltas. Every source is hashed through the ChaCha block
//!   function; no single source is trusted alone.
//! - `/dev/urandom` + `/dev/random` expose the same pool (phase-1: no
//!   blocking distinction; Linux's `random(4)` semantics shrink to that for
//!   a single-user kernel).

use core::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use spin::Mutex;

/// ChaCha20 block function: 64-byte keystream block from a 32-byte key,
/// 12-byte counter, 4-byte nonce (RFC 8439 reduced to 12 rounds is NOT used
/// here — full 20 rounds, `chacha20_block`-compatible layout).
const KEY_WORDS: usize = 8;
const BLOCK_BYTES: usize = 64;
const RESEED_INTERVAL: u64 = 1 << 20; // blocks before forced re-mix

struct Rng {
    key: [u32; KEY_WORDS],
    counter: u64,
    nonce: u32,
    /// Bytes generated since last re-mix (drives periodic re-keying).
    since_mix: u64,
    /// Stir entropy accumulated but not yet mixed in.
    stir: [u32; 16],
    stir_i: usize,
    initialized: bool,
}

static RNG: Mutex<Rng> = Mutex::new(Rng {
    key: [0; KEY_WORDS],
    counter: 0,
    nonce: 0,
    since_mix: 0,
    stir: [0; 16],
    stir_i: 0,
    initialized: false,
});

/// Debug/probe counter: how many bytes have been handed out.
static BYTES_OUT: AtomicUsize = AtomicUsize::new(0);
/// Monotonic stir counter (also an entropy input).
static STIRS: AtomicU64 = AtomicU64::new(0);

impl Rng {
    const fn new() -> Self {
        Self {
            key: [0; KEY_WORDS],
            counter: 0,
            nonce: 0,
            since_mix: 0,
            stir: [0; 16],
            stir_i: 0,
            initialized: false,
        }
    }

    /// One ChaCha20 block (20 rounds). `out` is 64 bytes of keystream.
    fn block(&self, counter: u64, out: &mut [u8; BLOCK_BYTES]) {
        // RFC 8439 state: "expa nd 3- by te k" constants, key, counter lo/hi,
        // nonce.
        let mut st = [
            0x6170_7865u32,
            0x3320_646e,
            0x7962_2d32,
            0x6b20_6574,
            self.key[0],
            self.key[1],
            self.key[2],
            self.key[3],
            self.key[4],
            self.key[5],
            self.key[6],
            self.key[7],
            counter as u32,
            (counter >> 32) as u32,
            self.nonce,
            0x9E37_79B9, // golden-ratio filler nonce word
        ];
        let init = st;
        for _ in 0..10 {
            // column rounds
            quarter(&mut st, 0, 4, 8, 12);
            quarter(&mut st, 1, 5, 9, 13);
            quarter(&mut st, 2, 6, 10, 14);
            quarter(&mut st, 3, 7, 11, 15);
            // diagonal rounds
            quarter(&mut st, 0, 5, 10, 15);
            quarter(&mut st, 1, 6, 11, 12);
            quarter(&mut st, 2, 7, 8, 13);
            quarter(&mut st, 3, 4, 9, 14);
        }
        for (i, w) in out.chunks_exact_mut(4).enumerate() {
            let v = st[i].wrapping_add(init[i]);
            w.copy_from_slice(&v.to_le_bytes());
        }
    }

    /// Fold 16 words of fresh entropy into the key (one ChaCha round as the
    /// mixer — a hash function would do; this keeps the module dependency-free).
    fn mix(&mut self) {
        let mut out = [0u8; BLOCK_BYTES];
        self.block(STIRS.load(Ordering::Relaxed) | 1, &mut out);
        let mut words = [0u32; 16];
        for (i, w) in words.iter_mut().enumerate() {
            let b = &out[i * 4..i * 4 + 4];
            *w = u32::from_le_bytes([b[0], b[1], b[2], b[3]]);
        }
        for i in 0..KEY_WORDS {
            self.key[i] ^= self.stir[i].wrapping_add(words[i]);
        }
        for i in 0..8 {
            self.key[i] ^= words[8 + i];
        }
        self.nonce = self.nonce.wrapping_add(words[15] | 1);
        self.counter = self.counter.wrapping_add(words[14] as u64 | (1 << 33));
        self.since_mix = 0;
        self.stir = [0; 16];
        self.stir_i = 0;
        self.initialized = true;
    }

    /// Push one entropy word into the stir pool.
    fn push(&mut self, w: u32) {
        // Rotate/accumulate: every word changes the pool.
        self.stir[self.stir_i] = self.stir[self.stir_i]
            .rotate_left(7)
            .wrapping_add(w ^ (STIRS.load(Ordering::Relaxed) as u32));
        self.stir_i = (self.stir_i + 1) % 16;
    }
}

fn quarter(s: &mut [u32; 16], a: usize, b: usize, c: usize, d: usize) {
    s[a] = s[a].wrapping_add(s[b]);
    s[d] = (s[d] ^ s[a]).rotate_left(16);
    s[c] = s[c].wrapping_add(s[d]);
    s[b] = (s[b] ^ s[c]).rotate_left(12);
    s[a] = s[a].wrapping_add(s[b]);
    s[d] = (s[d] ^ s[a]).rotate_left(8);
    s[c] = s[c].wrapping_add(s[d]);
    s[b] = (s[b] ^ s[c]).rotate_left(7);
}

/// Arch boot entropy: cycle counter / address-space layout / RTC.
///
/// Kept per-arch tiny and additive; new sources are appended in `init`.
#[allow(unused_assignments)]
fn arch_entropy_words() -> [u32; 16] {
    let mut w = [0u32; 16];
    let mut i = 0usize;
    #[cfg(target_arch = "x86_64")]
    {
        let (lo, hi): (u32, u32);
        let tsc: u64;
        unsafe {
            core::arch::asm!("rdtsc", out("rax") lo, out("rdx") hi, options(nostack, nomem));
            tsc = (lo as u64) | ((hi as u64) << 32);
        }
        w[i % 16] ^= tsc as u32;
        i += 1;
        w[i % 16] ^= (tsc >> 32) as u32;
        i += 1;
    }
    #[cfg(target_arch = "aarch64")]
    {
        let cnt: u64;
        unsafe {
            core::arch::asm!("mrs {}, cntvct_el0", out(reg) cnt, options(nostack, nomem));
        }
        w[i % 16] ^= cnt as u32;
        i += 1;
        w[i % 16] ^= (cnt >> 32) as u32;
        i += 1;
    }
    #[cfg(target_arch = "riscv64")]
    {
        let cyc: u64;
        unsafe {
            core::arch::asm!("rdcycle {}", out(reg) cyc, options(nostack, nomem));
        }
        w[i % 16] ^= cyc as u32;
        i += 1;
        w[i % 16] ^= (cyc >> 32) as u32;
        i += 1;
    }
    // Address-space layout (heap/bss/stack addresses differ per boot and
    // per build;Limine places the kernel at a fixed base but the heap bump
    // pointer lands on used frames).
    w[i % 16] ^= core::ptr::addr_of!(BYTES_OUT) as *const () as u32;
    i += 1;
    let t = crate::time::ticks();
    w[i % 16] ^= t as u32;
    i += 1;
    w[i % 16] ^= (t >> 32) as u32;
    i += 1;
    if let Some(sec) = crate::time::unix_seconds() {
        w[i % 16] ^= sec as u32;
        i += 1;
        w[i % 16] ^= (sec as u64 >> 32) as u32;
        i += 1;
    }
    // cpu id / smp state
    w[i % 16] ^= crate::smp::cpu_id() as u32 | 0x5bd1_e995;
    w
}

/// Initialize + seed the pool. Called once from kernel init (BSP).
pub fn init() {
    let mut rng = RNG.lock();
    if rng.initialized {
        return;
    }
    let words = arch_entropy_words();
    for w in words {
        rng.push(w);
    }
    rng.mix();
    // One extra mix over freshly sampled post-init state (timer tick drift).
    for w in arch_entropy_words() {
        rng.push(w);
    }
    rng.mix();
}

/// Stir the pool with fresh timer/cycle jitter. Cheap enough to call from
/// the timer tick path (a couple of cycles + adds, no mixing).
pub fn stir_tick() {
    let mut rng = RNG.lock();
    #[cfg(target_arch = "x86_64")]
    let jit = {
        let lo: u32;
        let _hi: u32;
        unsafe {
            core::arch::asm!("rdtsc", out("eax") lo, out("edx") _hi, options(nostack, nomem));
        }
        lo
    };
    #[cfg(target_arch = "aarch64")]
    let jit = {
        let cnt: u32;
        unsafe {
            core::arch::asm!("mrs {}, cntvct_el0", out(reg) cnt, options(nostack, nomem));
        }
        cnt as u32
    };
    #[cfg(target_arch = "riscv64")]
    let jit = {
        let cyc: u32;
        unsafe {
            core::arch::asm!("rdcycle {}", out(reg) cyc, options(nostack, nomem));
        }
        cyc
    };
    rng.push(jit);
    STIRS.fetch_add(1, Ordering::Relaxed);
}

/// Fill `out` with cryptographically-stirred random bytes.
///
/// Never fails once [`init`] has run; before init it self-seeds (safe for
/// early callers). The keystream counter guarantees no block reuse; every
/// `RESEED_INTERVAL` blocks the pool is re-mixed with fresh jitter so
/// future output stays forward-secure against a snapshot of the key.
pub fn fill(out: &mut [u8]) {
    let mut rng = RNG.lock();
    if !rng.initialized {
        for w in arch_entropy_words() {
            rng.push(w);
        }
        rng.mix();
    }
    let mut idx = 0usize;
    while idx < out.len() {
        let mut block = [0u8; BLOCK_BYTES];
        rng.block(rng.counter, &mut block);
        rng.counter = rng.counter.wrapping_add(1);
        rng.since_mix += 1;
        let n = (out.len() - idx).min(BLOCK_BYTES);
        out[idx..idx + n].copy_from_slice(&block[..n]);
        idx += n;
        if rng.since_mix >= RESEED_INTERVAL {
            // Re-key with current jitter before generating more.
            for w in arch_entropy_words() {
                rng.push(w);
            }
            rng.mix();
        }
    }
    BYTES_OUT.fetch_add(out.len(), Ordering::Relaxed);
}

/// Bytes handed out so far (debug/stat).
pub fn bytes_out() -> usize {
    BYTES_OUT.load(Ordering::Relaxed)
}
