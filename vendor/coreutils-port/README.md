# coreutils (uutils) porting notes for myos

Attempted cross-compiling [uutils/coreutils](https://github.com/uutils/coreutils) v0.10.0 for `x86_64-unknown-myos` using the patched myos sysroot.

## Quick repro

```sh
./scripts/build-sysroot.sh
./scripts/build-coreutils-myos.sh              # debug: echo,true,false
./scripts/build-coreutils-myos.sh --release
```

The build script:

1. Clones uutils into `user/uutils-coreutils/` (gitignored)
2. Runs `scripts/prepare-coreutils-patches.sh` — **fetches** `errno` and `libc` from crates.io and applies myos patches into `target/patched-crates/`
3. Copies `vendor/coreutils-port/cargo-config.toml` into uutils as `.cargo/config.toml` (`[patch.crates-io]`)

## Patch layout (in git)

Only myos-specific changes are committed — not whole upstream crates:

```
patches/coreutils/
  versions.env           # pinned errno/libc versions
  errno/
    myos.rs              # new backend
    lib-rs.patch
    sys-rs.patch
  libc/
    myos.rs              # C ABI shims for rustix
    lib-rs.patch
vendor/
  myos-rustix-stub/      # tiny myos-only crate (no upstream equivalent)
  coreutils-port/
    cargo-config.toml
```

Generated output (gitignored): `target/patched-crates/errno-*`, `target/patched-crates/libc-*`.

## What compiled

| Utilities | Build | Notes |
|-----------|-------|-------|
| `echo`, `true`, `false` | **yes** (dev + release) | Multicall `coreutils` ELF links |
| `cat`, `ls` | **no** | `uucore` `fs` feature needs real `rustix::fs` |
| Full `feat_common_core` | **no** | ~900 libc symbols, `hostname` crate, etc. |

Release builds need `CARGO_PROFILE_RELEASE_LTO=false` (handled by the build script; uutils sets `lto = "fat"`).

## Dependency blockers (in order hit)

### 1. `errno` — patched at fetch time

Upstream does not know `target_os = "myos"`. Patches add `patches/coreutils/errno/myos.rs`.

### 2. `libc` — patched at fetch time

Default libc is empty for unknown OSes. Patches add `patches/coreutils/libc/myos.rs` (syscall wrappers + ENOSYS stubs).

### 3. `rustix` — stubbed for minimal utilities

Full rustix needs ~900+ libc symbols. `vendor/myos-rustix-stub/` substitutes via `[patch.crates-io]` for echo/true/false only.

### 4. Beyond minimal utilities

- `hostname` — hard `compile_error!` for unknown OS
- `uucore::features::fs` — real `rustix::fs`, `AsFd` on `std::fs::File`

## Kernel / std gaps for a real port

| Layer | Missing today |
|-------|----------------|
| **Syscalls** | `stat`/`fstat`, `lseek`, `getdents`, env in process table, `rename`/`unlink`/`mkdir` |
| **VFS** | Writable FS, directories, symlinks, permissions, seek |
| **std** | `read_dir`, `metadata`, `File::seek` |
| **rustix** | Real backend or uucore changes to use `std::fs` on myos |

## Recommended next steps

1. Ship multicall `coreutils` with echo/true/false only; embed like other user ELFs.
2. Grow libc shims + real rustix (or patch uucore) for cat/ls.
3. Upstream `target_os = "myos"` to `errno` / `libc` when the ABI stabilizes.
