# coreutils (uutils) porting notes for myos

Cross-compiling [uutils/coreutils](https://github.com/uutils/coreutils) v0.10.0 for `x86_64-unknown-myos` using the patched myos sysroot.

## Quick repro

```sh
./toolchain/std/build-sysroot.sh
./ports/coreutils/prepare.sh   # fetch + patch errno, libc, rustix
./ports/coreutils/build.sh              # debug: echo,true,false
./ports/coreutils/build.sh --release
```

The build script:

1. Clones uutils into `user/uutils-coreutils/` (gitignored)
2. Runs `ports/coreutils/prepare.sh` — fetches `errno`, `libc`, and **crates.io `rustix`**; applies myos patches into `target/patched-crates/`
3. Copies `ports/coreutils/cargo-config.toml` into uutils as `.cargo/config.toml` (`[patch.crates-io]` + `rustix_use_libc`)

## Stack (real rustix + patched libc)

```
uutils/uucore → rustix (libc backend) → patched libc → myos.rs + rustix_compat.rs → ENOSYS / real syscalls
```

Groundwork strategy: satisfy **compile-time** libc/rustix surface with Linux-compatible types/constants and ENOSYS stubs. Runtime failures are acceptable until a utility actually needs a syscall.

## Patch layout (in git)

```
ports/coreutils/
  versions.env
  cargo-config.toml      # patches errno + libc + rustix; --cfg=rustix_use_libc
  crates/errno/ …
  crates/rustix/         # target_os = "myos" wiring (BorrowedFd, zero_msghdr, …)
  crates/getrandom/ hostname/ console/ filetime/ ctrlc/
  prepare.sh             # fetch + patch; also sed-fixes rustix fcntl call sites
ports/crates/libc/
  myos.rs                # real syscalls + ENOSYS macro
  rustix_compat.rs       # generated Linux-compat surface (see below)
  lib-rs.patch
  generate-libc-rustix-stubs.py
```

Regenerate `rustix_compat.rs` after changing the needed symbol set:

```sh
./ports/crates/libc/generate-libc-rustix-stubs.py
```

Generated output (gitignored): `target/patched-crates/{errno,libc,rustix}-*`.

`ports/coreutils/crates/myos-rustix-stub/` is **deprecated** — kept only for history; the build no longer patches it in.

## What compiled (release, x86_64)

| Utilities | Build | Runtime on myos |
|-----------|-------|-----------------|
| Most of `feat_common_core` + tier1 extras | **yes** (~11M ELF) | cat/ls/cp/rm/mkdir/du via `std::fs::read_dir` + SYS_LISTDIR |
| Dropped for now | `more`, `whoami`, `tac`, `tail`, `df`, `sync`, `test`, `split`, `tty`, `expr` | see `bins.txt` header |

## std changes for rustix

- `toolchain/std/patches/wire-myos.py` exposes `std::os::unix::{io,ffi}` on myos (re-exports `os::fd` / `os::myos::ffi`) so rustix’s libc backend can use `BorrowedFd` without `target_family = "unix"` (which breaks std’s own libc build).

## Recommended next steps

1. Implement remaining ENOSYS stubs as utilities need them (`symlink`, richer `stat`, …).
2. Revisit dropped utils (`tail`/`whoami`/`expr`/…) once onig/crossterm/getpw stubs exist.
3. Upstream `target_os = "myos"` pieces to `errno` / `libc` when the ABI stabilizes.
