# Rust `std` PAL for myos

This directory holds the myos Platform Abstraction Layer and the automation
that builds a **precompiled sysroot** for myos userspace on x86_64 and AArch64.

Target toolchain: `nightly-2026-07-26` (see root `rust-toolchain.toml`).

## Layout

| Path | Role |
|------|------|
| `pal/myos/` | PAL entry (`_start`), init, abort, syscall helpers |
| `sys/myos/` | `alloc` (brk), `fd`, `stdio`, shared `abi` |
| `os/myos/` | `OsStrExt` and fd re-exports |
| `patches/wire-myos.py` | Copies `rust-src` and inserts `target_os = "myos"` wiring |
| `../scripts/prepare-rust-std-myos.sh` | Patched library tree → `target/myos-sysroot` |
| `../scripts/build-sysroot.sh` | Precompile `std` for both triples into the sysroot |
| `../scripts/build-std-hello.sh` | Build smoke ELFs using the prebuilt sysroot |
| `../scripts/myos-sysroot-lib.sh` | Shared version stamp, install helpers, cargo wrappers |
| `../scripts/myos-rustc.sh` | `rustc` wrapper that points at the myos sysroot |
| `../targets/*.json` | Custom userspace triples (`x86_64-unknown-myos`, `aarch64-unknown-myos`) |
| `examples/hello/` | `println!("std ok")` smoke binary |

## Quick start

One-shot (CI uses this):

```sh
./scripts/build-std-hello.sh
```

That runs `build-sysroot.sh` (if stale), then builds `std-hello` for **both**
x86_64 and AArch64 without `-Z build-std` on the app crate.

### Manual steps

```sh
./scripts/build-sysroot.sh          # patch + precompile std (cached by version stamp)
./scripts/build-std-hello.sh        # smoke binaries → target/std-hello-*
```

Build a custom std program against the prebuilt sysroot:

```sh
export RUSTC_BOOTSTRAP=1
export MYOS_SYSROOT=$PWD/target/myos-sysroot
export RUSTC=$PWD/scripts/myos-rustc.sh

cargo +nightly-2026-07-26 build --release \
  -Z unstable-options -Z json-target-spec \
  --target targets/x86_64-unknown-myos.json \
  --manifest-path std/examples/hello/Cargo.toml
```

Root `.cargo/config.toml` sets `MYOS_SYSROOT`, `RUSTC_BOOTSTRAP`, and
`json-target-spec` for workspace builds; the `RUSTC` wrapper is exported by
the scripts above.

## Sysroot contents

`target/myos-sysroot` is a full nightly sysroot with:

- Patched `library/` sources (`target_os = "myos"` wiring)
- Target JSON specs under `lib/rustlib/<triple>.json`
- Prebuilt rlibs under `lib/rustlib/<triple>/lib/` for:
  - `x86_64-unknown-myos`
  - `aarch64-unknown-myos`

Invalidation: `.myos-sysroot-version` hashes the pinned nightly, `wire-myos.py`,
and all PAL/target files. Re-run `./scripts/build-sysroot.sh` after PAL changes.

## Kernel requirements

Syscall ABI matches `user/lib`: write (0), exit (1), read (3), close (4), brk (9).

| Arch | Entry | Syscall |
|------|-------|---------|
| x86_64 | argc/argv on stack (SysV) | `syscall` |
| AArch64 | argc in **x0**, argv in **x1** | `svc #0`, nr in **x8** |

The kernel embeds `/stdhello` from `target/std-hello-<triple>` on both architectures.
`user/heap` execs it after `alloc ok`.

## Status

Bring-up scope: **`println!("std ok")`** via patched `std` on x86_64-myos and
aarch64-unknown-myos (CI checks `"std ok"` on BIOS, UEFI, and AArch64).

Networking, filesystem, threads, and fork-aware `std` process support are still
stubs or unsupported paths in libstd.

Long term: upstream `target_os = "myos"` in Rust; until then publish versioned
sysroot tarballs keyed on the nightly + PAL hash.
