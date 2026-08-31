# coreutils (uutils) porting notes for myos

Cross-compiling [uutils/coreutils](https://github.com/uutils/coreutils) v0.10.0 for `x86_64-unknown-myos` using the patched myos sysroot.

## Quick repro

```sh
./scripts/build-sysroot.sh
./scripts/prepare-coreutils-patches.sh   # fetch + patch errno, libc, rustix
./scripts/build-coreutils-myos.sh              # debug: echo,true,false
./scripts/build-coreutils-myos.sh --release
```

The build script:

1. Clones uutils into `user/uutils-coreutils/` (gitignored)
2. Runs `scripts/prepare-coreutils-patches.sh` — fetches `errno`, `libc`, and **crates.io `rustix`**; applies myos patches into `target/patched-crates/`
3. Copies `vendor/coreutils-port/cargo-config.toml` into uutils as `.cargo/config.toml` (`[patch.crates-io]` + `rustix_use_libc`)

## Stack (real rustix + patched libc)

```
uutils/uucore → rustix (libc backend) → patched libc → myos.rs + rustix_compat.rs → ENOSYS / real syscalls
```

Groundwork strategy: satisfy **compile-time** libc/rustix surface with Linux-compatible types/constants and ENOSYS stubs. Runtime failures are acceptable until a utility actually needs a syscall.

## Patch layout (in git)

```
patches/coreutils/
  versions.env
  errno/ …
  libc/
    myos.rs              # real syscalls + ENOSYS macro
    rustix_compat.rs     # generated Linux-compat surface (see below)
    lib-rs.patch
  rustix/
    myos.patch           # target_os = "myos" wiring (BorrowedFd, zero_msghdr, …)
scripts/
  generate-libc-rustix-stubs.py
  prepare-coreutils-patches.sh   # also sed-fixes rustix fcntl call sites for myos
vendor/coreutils-port/
  cargo-config.toml      # patches errno + libc + rustix; --cfg=rustix_use_libc
```

Regenerate `rustix_compat.rs` after changing the needed symbol set:

```sh
./scripts/generate-libc-rustix-stubs.py
```

Generated output (gitignored): `target/patched-crates/{errno,libc,rustix}-*`.

`vendor/myos-rustix-stub/` is **deprecated** — kept only for history; the build no longer patches it in.

## What compiled (release, x86_64)

| Utilities | Build | Runtime on myos |
|-----------|-------|-----------------|
| `echo`, `true`, `false` | **yes** (~648K ELF) | works (PR #25 / #38 path) |
| `cat`, `ls` | **next** | needs real `read`/`getdents`/`rustix::fs`, not just stubs |
| Full `feat_common_core` | **no** | `getrandom`, `hostname`, broad POSIX surface |

## std changes for rustix

- `std/patches/wire-myos.py` exposes `std::os::unix::{io,ffi}` on myos (re-exports `os::fd` / `os::myos::ffi`) so rustix’s libc backend can use `BorrowedFd` without `target_family = "unix"` (which breaks std’s own libc build).

## Recommended next steps

1. CI: keep boot needles for echo/true/false; optionally add a **compile-only** job that builds real rustix + minimal features.
2. Implement syscalls as utilities need them (`lseek`, `getdents64`, env, …) — replace individual ENOSYS stubs in `myos.rs`.
3. Try `COREUTILS_FEATURES=cat` then `cat,ls`; fix the first real runtime failure.
4. Upstream `target_os = "myos"` pieces to `errno` / `libc` when the ABI stabilizes.
