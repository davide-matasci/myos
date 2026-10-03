# Instructions for AI agents

This file is for AI coding agents (and humans) working on myos. `CLAUDE.md`
imports it. Read `README.md` for the overview and the subsystem notes in
`docs/` before changing an area they cover.

## What myos is

A small Rust kernel (`#![no_std]`) that boots through Limine in QEMU on
**x86_64** (BIOS and UEFI), **AArch64** and **RISC-V** (riscv64imac), with a
VFS, kernel modules, preemptive scheduling, signals, networking (smoltcp in
userspace) and a C/Rust userspace built on newlib + libgloss. Ported programs
(oksh, sbase, ubase, uutils coreutils, ripgrep, tcc, vim, git, curl,
dropbear, ...) are fetched and cross-built at build time; the ones a boot
does not need are packages (`packages/`, published to a rolling GitHub
release, installed on the running system with `get-myos`).

Every change has to work on **all three arches**. Readability is a goal:
match the surrounding code's naming, idiom and comment density.

## Layout

| Path | What |
|------|------|
| `src/` | host launcher (`main.rs`: QEMU per arch, the test modes), image/initramfs assembly (`initramfs.rs`, `limine_image.rs`), the boot test's host side (`boot_test.rs`), the packages (`packages.rs`) |
| `build.rs` | builds the x86_64 kernel and the disk images; checks port artifacts per enabled feature |
| `kernel/` | the kernel (`arch/`, `task/`, `fs/`, `user/`, `modules/`, `dt.rs` for the device tree) |
| `modules/` | loadable kernel modules and their `#[repr(C)]` ABI (`modules/abi`); the console module also holds the keymaps and the `ps2-scancode` crate |
| `user/` | native userspace: Rust (init, netd, smokes, `myos_user` lib), C (`user/c`: hello and the test smokes) and the boot tests' runner (`user/tests`); one `port.env` per program |
| `toolchain/` | newlib + libgloss/myos, the Rust `std` port (`toolchain/std`); both are ports too (`port.env`, kind `toolchain`) |
| `ports/<name>/` | one directory per ported program in the image: `port.env` (descriptor, `docs/ports.md`), `versions.env` (pin), `fetch.sh`, `build.sh`, `*.myos.patch`, notes |
| `packages/<name>/` | the same, for programs CI builds and publishes but the image does not carry (vim, git, lynx, lua, make, os-test, ncurses; `get-myos` installs them, `docs/packages.md`); a port moves between the two by moving its directory |
| `linux-compat/` | optional Linux syscall layer userspace (launcher, musl build, tests, `get-alpine`) |
| `scripts/` | CI scripts, registry, `ports.sh` (reads the descriptors), thin wrappers for port builds |
| `targets/` | custom Rust target specs for userspace |
| `docs/` | design notes per subsystem (signals, sockets, Linux layer, ...) |
| `target/` | all build output and fetched sources (never committed) |

## Building

Prerequisites are listed in `README.md` (QEMU for 3 arches, clang + lld,
make, git, curl, rsync, patch, ...). `rust-toolchain.toml` pins the nightly;
don't change it except through `scripts/bump-nightly.sh`.

```sh
./toolchain/newlib/build.sh          # newlib + libgloss for the 3 arches (once)
./ports/<name>/build.sh              # a port; skips itself when up to date
cargo build                          # x86_64 kernel + images (target/*.img)
cargo run -- [uefi|aarch64|riscv64]  # build and boot in QEMU (default: x86 BIOS)
```

- `build.rs` runs the build script of every port of the image itself when
  its outputs are missing (or fails with that script's error). The
  initramfs packer refuses a missing file instead of silently leaving it
  out, and re-packs the images when a packed file under `target/` changes.
  What is in the image is decided by the port descriptors (`port.env`,
  `docs/ports.md`): every `ports/`, `user/` and `toolchain/` directory
  with one is in, `packages/` directories are built but not shipped. There
  are no per-port Cargo features.
- `--features linux_compat` adds the optional Linux layer; build its pieces
  first with `./linux-compat/build.sh` (see `docs/linux-compat.md`).
- The aarch64 and riscv64 kernels are built by the launcher; to just
  type-check: `cargo check -p kernel --target {x86_64-unknown-none,aarch64-unknown-none-softfloat,riscv64imac-unknown-none-elf}`.
- First builds are slow (fetching and cross-building ports, newlib, the
  `std` sysroot). Everything lands in `target/`.

## Testing

The real test is booting: `test-mini` / `test-full` boot headless, log in
and run a test list **in the guest** (`user/tests`, the ports' `test.sh`),
while the host watches the console for `TEST <name> PASS|FAIL` lines
(`src/boot_test.rs`, `docs/testing.md`). QEMU runs under TCG, so a boot
takes minutes.

```sh
cargo run -- test-mini                     # quick list (what PR CI runs)
cargo run -- aarch64 test-mini             # also riscv64, uefi
cargo run -- test-full                     # full list: + packages, HTTPS, SSH, Alpine, curated os-test (must be 100%)
cargo run -- packages                      # the package tarballs + indexes (target/packages/)
scripts/local-ci.sh [bios|uefi|aarch64|riscv64] [mini|full]   # the same with OOM/TCG settings for a loaded host
cargo test -p ps2-scancode -p ext2fs      # host unit tests (ext2fs needs e2fsprogs)
```

- Test on every arch you could have affected; arch-specific code needs all
  three. Don't over-test: a mini run per affected arch is usually enough
  locally, CI does the rest.
- New behavior gets a test: a function and a `t` line in a section of
  `user/tests/`, a port's `test.sh` (`PORT_TEST` in its `port.env`; a
  package's test runs in the full mode after the install), a smoke program
  under `user/c`, or an os-test in the curated lists
  (`packages/os-test/overlay/misc/*.tests`). A test's output is shown only
  when it fails: keep passing tests quiet.
- The full mode needs the network (`https://example.com/`, the Alpine
  mirror for `get-alpine`); the packages come from the build itself, served
  by the launcher to the guest (`docs/packages.md`). In a sandbox with a
  TLS-intercepting proxy, append its CA to `target/cacert.pem` for local
  runs only and restore it afterwards; never commit it.

## CI (GitHub Actions)

- `ci.yml` calls `ci-ports.yml` (cross-builds each port, cached as OCI
  artifacts on GHCR keyed by a hash of its inputs, `scripts/ci-registry.sh`;
  the two toolchain jobs run only when the registry lacks them) and
  `ci-runtime.yml` (build job → boot jobs).
- Pull requests run **test-mini** on bios, uefi, aarch64 and riscv64. The
  Linux layer's musl pieces are built and cached like a port
  (`linux-compat`) in every run.
- **Full boot** (`test-full` in all four boot jobs, every image with the
  Linux layer built in by `MYOS_CI_FEATURES=linux_compat`, every package
  installed from the build's own mirror) runs daily on master and on
  demand: dispatch `ci.yml` with `full_boot: true` on the branch. Run it
  for kernel, libc, port or CI changes that the mini list does not cover.
- On master (a push or the daily run) the **iso** job builds the x86_64
  hybrid ISO with the Linux layer in it and the **publish** job uploads it
  and the packages to the rolling GitHub release `rolling`, get-myos's
  default mirror (`docs/packages.md`). The ISO is not boot-tested: the bios
  and uefi boots carry the same files.
- CI runs in the `myos-ci` container (`Dockerfile`, `build-ci-image.yml`).
- Changing a port's pin or build script changes its cache key; CI rebuilds
  it. The ports matrix of `ci-ports.yml` and the pack/assert lists of the
  build and boot jobs come from the descriptors. Adding a port touches
  `ports/<name>/` (with its `port.env` and `test.sh`),
  `scripts/myos-c-userspace-lib.sh` (hash + freshness functions) and
  `THIRD_PARTY_NOTICES.md`; see "Adding a port" in `docs/ports.md` and
  follow an existing port such as `ports/lua`.

## Rules and conventions

- **Append-only ABIs**: native syscall numbers and the module `KernelApi`
  (`modules/abi`) only grow; never renumber or reorder.
- **Optional stays optional**: features off by default (like `linux_compat`)
  must not change a default build; gate their code with `cfg(feature)`.
- **No vendored upstream sources**: ports pin a version/revision (and
  sha256 for tarballs) in `versions.env` and are fetched into `target/`.
  myos changes to upstream code are `*.myos.patch` files or small compat
  headers in the port directory.
- **No new prebuilt binaries in the repo or the image** from third parties
  (e.g. Linux packages are downloaded at run time by `get-alpine`, never
  shipped).
- Keep docs current: update `README.md` / `docs/*.md` when behavior,
  limits, commands or CI change.
- Kernel limits (heap, fds, tasks, mmap windows) are documented in the
  relevant `docs/` page; change both together.
- Commits: one topic each, subject `area: what changed` (imperative or
  descriptive, no trailing period), body explaining why. One branch and one
  PR per change; never force-push a shared branch.

## License compliance

myos's own code is `MIT OR Apache-2.0` (`LICENSE-MIT`, `LICENSE-APACHE`).
External software keeps its own license, listed in `THIRD_PARTY_NOTICES.md`;
the copyleft license texts are in `licenses/` (`cargo run -- iso` copies
both into the ISO). **License compliance must be maintained whenever you
add, update or vendor anything external** (a port, a crate, a code snippet,
a data file, firmware, a downloaded binary):

- Check the upstream license before integrating, and that it permits what
  myos does with it (building, patching, and redistributing binaries inside
  the boot images). Prefer permissive licenses; flag copyleft (GPL, LGPL,
  MPL, ...) and anything unusual to the maintainer before adding it.
- **Update `THIRD_PARTY_NOTICES.md` in the same change** when you add,
  bump or remove a component (re-check the upstream license on every
  bump), and add the full text to `licenses/` for a new copyleft license.
- Copyleft components already ship in the images (git, lynx, GNU make,
  TinyCC, the Mozilla CA bundle; see the notices file). Distributing their
  binaries requires the corresponding source: keep the pinned upstream
  source reference (`versions.env`) and every myos patch in the repo, so
  each image is reproducible from source.
- Keep upstream copyright and license notices intact in anything copied or
  patched; never strip headers. Code copied into the repo (even a small
  function) keeps its notice, gets a comment naming its origin and
  license, and is listed in `THIRD_PARTY_NOTICES.md`.
- Don't copy code from sources with unknown or incompatible licenses
  (including code of unclear origin), and don't relicense anything:
  myos patches and files derived from upstream stay under the upstream
  license, not myos's.
- Rust crate dependencies: check their license (and their dependencies')
  when adding one.
