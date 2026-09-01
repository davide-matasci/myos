# myos ripgrep port — patch audit

Upstream is **not** vendored. Build scripts fetch:

| Component | Pin | Location |
|-----------|-----|----------|
| ripgrep | `15.2.0` (`e89fff89…`) | `target/ripgrep-src` via `scripts/fetch-ripgrep.sh` |
| PCRE2 | `pcre2-10.45` | `target/pcre2-src` via `scripts/fetch-pcre2.sh` |
| pcre2-sys | `0.2.10` (crates.io) | cargo registry; `build.rs` replaced at prepare time |

## Files in this directory

- `versions.env` — pins above
- `pcre2-headers/{config.h,pcre2.h}` — configure-generated headers used when
  cross-compiling PCRE2 for myos (JIT left undefined). Origin: the same
  generated headers shipped inside `pcre2-sys` 0.2.10’s `upstream/include`
  (PCRE2 project license). Kept here so the C build does not depend on a
  prior cargo fetch of pcre2-sys.
- `pcre2-sys-build.rs` — drop-in `build.rs` for pcre2-sys 0.2.10 that:
  1. Links a prebuilt `libpcre2-8` when `PCRE2_LIB_DIR` + `PCRE2_INCLUDE_DIR` are set
  2. Disables JIT on `*myos*` targets

`scripts/build-pcre2-myos.sh` compiles `pcre2_jit_compile.c` **without**
`SUPPORT_JIT` so the public `pcre2_jit_*_8` symbols exist as stubs
(required by the Rust `pcre2` crate) but no executable JIT is emitted.

## Ripgrep source mutations (prepare-time, not committed)

Applied under `target/ripgrep-src` only:

1. Append `[profile.release-myos]` (opt-level=z, fat LTO) for image size
2. Append `[patch.crates-io]` → myos-patched `libc` 0.2.189 (shared with uutils)
3. Copy `vendor/ripgrep-port/cargo-config.toml` → `.cargo/config.toml`
   (myos rustc wrapper, newlib `-lc -lgloss`, `--allow-multiple-definition`
   because Rust `libc` and libgloss both define `lstat`)

No edits to ripgrep’s Rust sources are required for the default + `pcre2`
feature set on nightly-2026-07-26.

## Runtime / kernel

- ELF registered as `/c/rg` on coreutilsfs (separate from uutils multicall)
- `MAX_INIT_PAGES` raised to 1024 so the ~721-page PT_LOAD span loads

## Runtime fixes (myos std + ignore)

CI QEMU smoke initially saw `rg` abort with exit **101** on file search while
`rg --version` worked. Root causes:

1. **`Instant::now()` panics** on myos (`std` used the unsupported time backend).
   Fixed by `std/sys/time/myos.rs` (wired in `wire-myos.py`).
2. **`ignore::WalkBuilder` never yields file paths** on non-unix/non-windows:
   `DirEntryRaw::from_path` returned "unsupported platform".
   Fixed at prepare time by `scripts/patch-ignore-myos.py` (uses `fs::metadata`
   / `SYS_STAT`).
3. **`fs::metadata` was `unsupported()`** (`FileAttr(!)`). Implemented via
   `SYS_STAT` in `std/sys/fs/myos.rs` + `abi::stat`.

Heap smoke uses `-j1 --no-mmap --no-config` (myos is `singlethread`, no mmap).
