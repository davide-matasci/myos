# Rust `std` PAL skeleton for myos

This directory mirrors what belongs in a patched Rust tree at
`library/std/src/sys/pal/myos/`. Copy or symlink it there, then apply the
wiring patches below and build with `-Z build-std=std,panic_abort`.

See also the [OSDev wiki](https://wiki.osdev.org/Porting_Rust_standard_library).

## 1. Allow the OS in `library/std/build.rs`

Add `|| target_os == "myos"` to the supported-OS list (same block as `uefi`,
`hermit`, …).

## 2. Wire the PAL in `library/std/src/sys/pal/mod.rs`

```rust
} else if #[cfg(target_os = "myos")] {
    mod myos;
    pub use self::myos::*;
```

## 3. Copy this tree

```sh
cp -r std/pal/myos "$RUST_SRC/library/std/src/sys/pal/myos"
```

Set `RUST_SRC` to your pinned nightly source (same version as `rust-toolchain.toml`).

## 4. Build a std program (host cross-compile)

From this repo after patching Rust:

```sh
export RUSTC_BOOTSTRAP=1
cargo +nightly build \
  -Z build-std=std,panic_abort \
  --target targets/x86_64-unknown-myos.json \
  --manifest-path std/examples/hello/Cargo.toml
```

The myos kernel must already provide syscall **9 (`brk`)** and the usual fd/process
API documented in the root `README.md`.

## Status

| PAL module | Status |
|------------|--------|
| `alloc.rs` | Bump `GlobalAlloc` on `brk` (matches `user/lib`) |
| `os.rs` | Raw `write`/`read`/`open`/`close`/`exit` syscalls |
| `args.rs` | argc/argv from `_start` stack (x86 SysV) |
| `thread_local_key.rs` | Single-threaded stub |
| `start.rs` | `_start` → `std::rt` (needs full std link) |

Full `std` (threads, filesystem, networking) needs more kernel features (`mmap`,
`clock_gettime`, …). This skeleton targets **`println!("std ok")` bring-up**.
