//! The std heap frees: far more memory than the machine has is allocated and
//! freed in turn, in large blocks (whole mappings of their own) and small
//! ones (dlmalloc's chunks), grown with `realloc`, and the process stays
//! small (its size in `/proc/<pid>/status`, `docs/proc.md`).
#![no_main]

/// The process's virtual size in KiB, the status line's 8th field.
fn size_kib() -> u64 {
    let pid = std::process::id();
    let line = std::fs::read_to_string(format!("/proc/{pid}/status")).unwrap_or_default();
    line.split_whitespace().nth(7).and_then(|s| s.parse().ok()).unwrap_or(0)
}

fn fail(what: &str) -> ! {
    println!("[ FAIL ] std alloc: {what}");
    std::process::exit(1);
}

#[unsafe(no_mangle)]
pub extern "C" fn main() {
    const ROUNDS: usize = 256;
    let before = size_kib();
    let mut total = 0usize;
    for round in 0..ROUNDS {
        // 4 MiB, touched so its pages are real.
        let big = vec![round as u8; 4 << 20];
        if big[(4 << 20) - 1] != round as u8 {
            fail("big block contents");
        }
        total += big.len();
        // Small blocks, grown by realloc.
        let mut small: Vec<Vec<u8>> = (0..256).map(|i| vec![i as u8; 64 + i]).collect();
        for v in small.iter_mut() {
            v.extend_from_slice(&[7; 512]);
        }
        if small.iter().enumerate().any(|(i, v)| v[0] != i as u8 || v.len() != 64 + i + 512) {
            fail("small block contents");
        }
        total += small.iter().map(|v| v.len()).sum::<usize>();
    }
    let after = size_kib();
    // A few MiB of dlmalloc's own on top of where it started, not the 1 GiB
    // that went through it.
    if before == 0 || after > before + 16 * 1024 {
        fail(&format!("size {before} KiB before, {after} KiB after {} MiB", total >> 20));
    }
}
