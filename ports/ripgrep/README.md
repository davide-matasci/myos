# myos ripgrep port — patch audit

Upstream is **not** vendored. Build scripts fetch:

| Component | Pin | Location |
|-----------|-----|----------|
| ripgrep | `15.2.0` (`e89fff89…`) | `target/ripgrep-src` via `ports/ripgrep/fetch.sh` |
| PCRE2 | the pcre2 package's (`packages/pcre2/versions.env`) | `target/pcre2-<arch>`, built by `packages/pcre2/build.sh` |
| pcre2-sys | `0.2.10` (crates.io) | cargo registry; `build.rs` replaced at prepare time |

## Files in this directory

- `versions.env` — pins above
- `pcre2-sys-build.rs` — drop-in `build.rs` for pcre2-sys 0.2.10 that:
  1. Links a prebuilt `libpcre2-8` when `PCRE2_LIB_DIR` + `PCRE2_INCLUDE_DIR` are set
  2. Disables JIT on `*myos*` targets

ripgrep links the pcre2 package's `libpcre2-8.a` (`PORT_DEPS`, one PCRE2
tree and build for it and nginx): `pcre2_jit_compile.c` is compiled there
**without** `SUPPORT_JIT`, so the public `pcre2_jit_*_8` symbols exist as
stubs (required by the Rust `pcre2` crate) but no executable JIT is emitted.

## Ripgrep source mutations (prepare-time, not committed)

Applied under `target/ripgrep-src` only:

1. Append `[profile.release-myos]` (opt-level=z, fat LTO) for image size
2. Append `[patch.crates-io]` → myos-patched `libc` 0.2.189 (shared with uutils)
3. Copy `ports/ripgrep/cargo-config.toml` → `.cargo/config.toml`
   (myos rustc wrapper, newlib `-lc -lgloss`, `--allow-multiple-definition`
   because Rust `libc` and libgloss both define `lstat`)

No edits to ripgrep’s Rust sources are required for the default + `pcre2`
feature set on nightly-2026-07-26.

## Runtime / kernel

- The ELF is `/bin/coreutils/rg` (its own file, not a name of the uutils multicall)
- Its ~721-page PT_LOAD span is within the loader's cap (`MAX_ELF_PAGES`, `kernel/src/user/mod.rs`)

## Runtime fixes (myos std + ignore)

CI QEMU smoke initially saw `rg` abort with exit **101** on file search while
`rg --version` worked. Root causes:

1. **`Instant::now()` panics** on myos (`std` used the unsupported time backend).
   Fixed by `toolchain/std/sys/time/myos.rs` (wired in `wire-myos.py`).
2. **`ignore::WalkBuilder` never yields file paths** on non-unix/non-windows:
   `DirEntryRaw::from_path` returned "unsupported platform".
   Fixed at prepare time by `ports/ripgrep/patch-ignore-myos.py` (uses `fs::metadata`
   / `SYS_STAT`).
3. **`fs::metadata` was `unsupported()`** (`FileAttr(!)`). Implemented via
   `SYS_STAT` in `toolchain/std/sys/fs/myos.rs` + `abi::stat`.

Heap smoke uses `-j1 --no-mmap --no-config` (rg mmap is optional); `test.sh` searches a
tree with `-j4`, the walker and the searchers on `std::thread`.
