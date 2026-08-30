# coreutils (uutils) porting notes for myos

Attempted cross-compiling [uutils/coreutils](https://github.com/uutils/coreutils) v0.10.0 for `x86_64-unknown-myos` using the patched myos sysroot.

## Quick repro

```sh
./scripts/build-sysroot.sh
./scripts/build-coreutils-myos.sh              # dev: echo,true,false
./scripts/build-coreutils-myos.sh --release    # needs -C lto=off (see script)
```

The script clones uutils into `user/uutils-coreutils/` (gitignored) and applies `[patch.crates-io]` from `vendor/coreutils-port/cargo-config.toml`.

## What compiled

| Utilities | Build | Notes |
|-----------|-------|-------|
| `echo`, `true`, `false` | **yes** (dev profile) | Multicall `coreutils` ELF links; ~22 MiB debug (ICU/fluent/clap) |
| `cat`, `ls` | **no** | `uucore` `fs` feature needs real `rustix::fs` + `AsFd` plumbing |
| Full `feat_common_core` | **no** | Hundreds of libc/rustix symbols, `hostname` crate, etc. |

Dev build command that succeeded:

```sh
cargo +nightly-2026-07-26 build \
  --target ../../targets/x86_64-unknown-myos.json \
  --no-default-features --features echo,true,false \
  --bin coreutils
```

Release fails with uutils' default `lto = "fat"` on the myos sysroot (`Can't find section .llvmbc`). Use `RUSTFLAGS='-C lto=off'` until LTO is sorted out.

## Dependency blockers (in order hit)

### 1. `errno` crate — fixed (vendor patch)

Upstream does not know `target_os = "myos"`. Added `vendor/errno/src/myos.rs` with a process-global errno.

### 2. `libc` crate — partial (vendor patch)

Default libc is empty for unknown OSes. Added `vendor/libc/src/myos.rs` with:

- Linux-like types/constants (subset)
- Syscall wrappers for: read, write, open, close, pipe, dup2, fork, wait, brk, fstat (stub), getcwd, clock_gettime
- ENOSYS stubs for ~40 other functions

Real rustix still needs **900+** additional constants/types (`SIG*`, `msghdr`, `statfs`, `*at` syscalls, …).

### 3. `rustix` — stubbed for minimal utilities

Full rustix cannot compile against the minimal libc. For echo/true/false we substitute `vendor/myos-rustix-stub/` via `[patch.crates-io]`.

Utilities that use `uucore` with `feature = "fs"` (cat, ls, cp, …) **require a real rustix + richer libc**, not the stub.

### 4. Other crates (seen when enabling cat/ls)

- `hostname` — `compile_error!("Unsupported target OS!")`
- `uucore::features::fs` — expects `rustix::fs::Stat`, `AsFd` on `std::fs::File`, etc.

## Kernel / std gaps implied by a *real* port

To go beyond echo/true/false without stubs:

| Layer | Missing today |
|-------|----------------|
| **Syscalls** | `stat`/`fstat` metadata, `lseek`, `getdents`/`listdir` as libc API, env vars in process table, `rename`/`unlink`/`mkdir`, real errno per thread |
| **VFS** | Writable FS, directories, symlinks, permissions, seek |
| **std** | `std::fs::read_dir`, `metadata`, `File::seek`, more `OpenOptions` |
| **libc** | Full Linux-compatible header surface (or upstream myos target in `libc`) |
| **rustix** | Either upstream myos support or drop rustix from uucore on myos |

## Recommended next steps

1. **Short term:** Ship multicall `coreutils` with echo/true/false only; embed like other user ELFs.
2. **Medium term:** Extend myos libc + drop rustix stub for a thin `rustix` backend wired to myos syscalls (or patch uucore to use `std::fs` on myos).
3. **Long term:** Upstream `target_os = "myos"` patches to `libc`, `errno`, and optionally `rustix`; grow syscall set as utilities demand it.

## Vendor layout

```
vendor/
  errno/              # errno crate + myos.rs
  libc/               # libc crate + myos.rs shim
  myos-rustix-stub/   # empty rustix for non-fs utilities
  coreutils-port/
    cargo-config.toml # patches + myos sysroot for uutils build
```
