# Upstreaming `target_os = "myos"` to Rust

This directory tracks the path from the in-repo **patched sysroot** workflow to
an upstream **Tier 3** `*-unknown-myos` target with `std` in `rust-lang/rust`.

## Current state (out-of-tree)

| Piece | Location | Upstream equivalent |
|-------|----------|---------------------|
| Target JSON specs | `targets/*-unknown-myos.json` | `compiler/rustc_target/src/spec/*.rs` (or keep JSON + `-Z json-target-spec` initially) |
| PAL + sys wiring | `std/pal/`, `std/sys/myos/`, `std/os/myos/` | `library/std/src/sys/pal/myos/`, etc. |
| libstd cfg patches | `std/patches/wire-myos.py` | Direct edits in `library/std/src/**/mod.rs` |
| Prebuilt sysroot | `scripts/build-sysroot.sh` | Unnecessary once Rust ships target + optional prebuilt std |
| Platform docs | `platform-support/myos.md` | `src/doc/rustc/src/platform-support/myos.md` |

Today consumers use:

```sh
./scripts/fetch-sysroot.sh          # or build-sysroot.sh
./scripts/build-std-hello.sh
```

With upstream `target_os = "myos"`, the goal is:

```sh
rustup target add x86_64-unknown-myos   # future
cargo build --target x86_64-unknown-myos
```

## Tier 3 checklist (rust-lang/rust)

Follow the [Target Tier Policy](https://doc.rust-lang.org/rustc/target-tier-policy.html).

### 1. Compiler target(s)

Add at least:

- `x86_64-unknown-myos`
- `aarch64-unknown-myos` (softfloat, matching the kernel)

Start from `targets/*.json` in this repo. Hermit’s specs (`x86_64-unknown-hermit`,
`aarch64-unknown-hermit`) are good templates for `compiler/rustc_target/src/spec/`.

Requirements:

- Cross-compile from any host with LLVM backend support (x86_64 + AArch64 ✓)
- `panic = abort`, `rust-lld`, PIE userspace ELFs
- No breakage to other targets

### 2. `library/std` PAL

Copy PAL sources from this repo (single source of truth until merge):

- `std/pal/myos/` → `library/std/src/sys/pal/myos/`
- `std/sys/myos/` → `library/std/src/sys/myos/` (+ `alloc/myos.rs`, `fd/myos.rs`, `stdio/myos.rs`)
- `std/os/myos/` → `library/std/src/os/myos/`

Apply the wiring currently done by `wire-myos.py` directly in upstream files
(`std/build.rs`, `sys/pal/mod.rs`, `os/mod.rs`, fd cfgs, etc.).

Generate a reviewable diff locally:

```sh
./scripts/check-wire-myos.sh           # verify patch applies on pinned nightly
./scripts/export-upstream-patch.sh     # -> target/myos-upstream-library.patch
```

### 3. Platform support documentation

Submit `platform-support/myos.md` (draft in this repo) to:

`src/doc/rustc/src/platform-support/myos.md`

Also update `SUMMARY.md` and `platform-support.md` in that tree.

The doc must explain:

- Cross-compilation from Linux hosts
- How to run ELFs under QEMU (link to myos README / CI)
- That `std` is bring-up scope today; full POSIX subset is not claimed

### 4. Maintainer & CI

- Designated target maintainers (CC on target-specific PRs)
- myos CI boots BIOS/UEFI/aarch64/riscv64 and types `heap` for the same `std ok` needles on every arch
- Point upstream reviewers at https://github.com/davide-matasci/myos CI

### 5. Suggested PR sequence

1. **Tier 3 target only (`#![no_core]`)** — teach rustc about `x86_64-unknown-myos` with no `std` changes (breaks the `cc`/`libc`/`std` cycle per tier policy).
2. **`std` PAL + wiring** — port PAL files + `wire-myos.py` edits as native upstream changes.
3. **AArch64 triple** — second target spec + PAL is already multi-arch.
4. **Prebuilt std** (optional) — Rust project may still expect `-Z build-std` for tier 3; myos can keep local/CI sysroot tarballs until then.

## After upstream merge

In myos we can:

1. Drop `wire-myos.py` once wiring lives in rust-lang/rust
2. Retain `scripts/package-sysroot.sh` for local installs and CI artifact caching
3. Switch `MYOS_NIGHTLY` pin to a stable channel when myos reaches tier 2 (long-term)
4. Keep `myos_user` for minimal `#![no_std]` ELFs embedded by the kernel

## References

- [Porting Rust standard library (OSDev)](https://wiki.osdev.org/Porting_Rust_standard_library)
- [Hermit platform support doc](https://doc.rust-lang.org/rustc/platform-support/hermit.html)
- [Adding a new target (rustc dev guide)](https://rustc-dev-guide.rust-lang.org/building/new-target.html)
- In-repo: `std/pal/README.md`, `std/toolchain/config.toml.example`
