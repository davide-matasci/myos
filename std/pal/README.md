# Rust `std` PAL for myos

This directory holds the myos Platform Abstraction Layer and the automation
that patches a pinned nightly Rust tree for `-Z build-std`.

Target toolchain: `nightly-2026-07-26` (see root `rust-toolchain.toml`).

## Layout

| Path | Role |
|------|------|
| `pal/myos/` | PAL entry (`_start`), init, abort, syscall helpers |
| `sys/myos/` | `alloc` (brk), `fd`, `stdio`, shared `abi` |
| `os/myos/` | `OsStrExt` and fd re-exports |
| `patches/wire-myos.py` | Copies `rust-src` and inserts `target_os = "myos"` wiring |
| `../scripts/prepare-rust-std-myos.sh` | Builds a custom sysroot under `target/myos-sysroot` |
| `../scripts/myos-rustc.sh` | rustc wrapper that points at the patched sysroot |
| `examples/hello/` | `println!("std ok")` smoke binary |

## Quick start

```sh
./scripts/prepare-rust-std-myos.sh

export RUSTC_BOOTSTRAP=1
export MYOS_SYSROOT=$PWD/target/myos-sysroot
export RUSTC=$PWD/scripts/myos-rustc.sh

cargo +nightly-2026-07-26 build \
  -Z build-std=std,panic_abort \
  -Z build-std-features=compiler-builtins-mem \
  -Z unstable-options \
  -Z json-target-spec \
  --target targets/x86_64-unknown-myos.json \
  --manifest-path std/examples/hello/Cargo.toml
```

The resulting ELF is at `std/examples/hello/target/x86_64-unknown-myos/debug/std-hello`.

## Kernel requirements

Syscall ABI matches `user/lib`: write (0), exit (1), read (3), close (4), brk (9).
x86_64 `_start` expects SysV argc/argv on the stack (same as existing user ELFs).

## Status

Bring-up scope: **`println!("std ok")`** via patched `std` on x86_64-myos.
Networking, filesystem, threads, and fork-aware `std` process support are still stubs
or unsupported paths in libstd.

Long term, publish a versioned **sysroot** per pinned nightly so consumers use
`--sysroot=` instead of patching locally; PAL source stays in this repo.
