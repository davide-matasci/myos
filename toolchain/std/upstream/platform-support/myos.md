# *-unknown-myos

**Tier:** 3

**Maintainers:** myos developers ([myos repository](https://github.com/davide-matasci/myos))

## Overview

`*-unknown-myos` targets compile Rust userspace programs for
[myos](https://github.com/davide-matasci/myos), a small `no_std` kernel with
a minimal syscall ABI (write, exit, read, close, brk, open, fork, exec, wait,
listdir).

Available triples (proposed):

| Triple | Arch | Notes |
|--------|------|-------|
| `x86_64-unknown-myos` | x86_64 | SysV `syscall`, argc/argv on stack at `_start` |
| `aarch64-unknown-myos` | AArch64 softfloat | `svc #0`, argc in x0 / argv in x1 at `_start` |

These targets support `std` at bring-up scope (`println!`, heap via `brk`, basic
stdio). Filesystem, process, thread, network, and time APIs remain largely
unsupported in libstd until the kernel and PAL grow.

## Building

### Host requirements

- Rust nightly (see myos `rust-toolchain.toml` for the pinned version used in CI)
- `rust-src` component
- `rust-lld` (via `RUSTC_BOOTSTRAP=1` or bootstrap config)

### Cross-compilation (today, out-of-tree PAL)

Until this target lands in upstream Rust, build the myos patched sysroot:

```sh
git clone https://github.com/davide-matasci/myos
cd myos
./toolchain/std/fetch-sysroot.sh
cargo +nightly build -Z unstable-options -Z json-target-spec \
  --target targets/x86_64-unknown-myos.json \
  --manifest-path toolchain/std/examples/hello/Cargo.toml
```

See `toolchain/std/toolchain/config.toml.example` for a standalone app manifest.

### Cross-compilation (after upstream merge)

Expected workflow once the target is in `rust-lang/rust`:

```sh
rustup toolchain install nightly --component rust-src
cargo build -Z build-std=std,panic_abort --target x86_64-unknown-myos
```

Prebuilt std artifacts may remain a private/local sysroot until tier 2 promotion.

## Testing

Run myos under QEMU; CI on every arch types `heap` at `$` after slim `/ok`
and checks the same serial needles including `std ok`:

```sh
./toolchain/std/build-std-hello.sh
cargo build
cargo run -- --ci              # x86 BIOS (+ heap at $)
cargo run -- uefi --ci         # x86 UEFI (+ heap at $)
cargo run -- aarch64 --ci      # AArch64 (+ heap at $)
cargo run -- riscv64 --ci      # RISC-V64 (+ heap at $)
```

User ELFs are position-independent executables linked with `rust-lld`.

## Syscall ABI

Documented in the myos README. Numbering matches the in-kernel dispatcher and
the `myos_user` crate used by `#![no_std]` utilities.

## Related tools

- **myos_user** — minimal `#![no_std]` syscall layer for kernel-embedded tools
  (still recommended for tiny ELFs without libstd)
- **Prebuilt sysroot tarballs** — built locally or attached to CI workflow runs (`toolchain/std/package-sysroot.sh`); not published as public releases
