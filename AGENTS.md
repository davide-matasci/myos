# Instructions for AI agents

This file is for AI coding agents (and humans) working on myos. `CLAUDE.md`
imports it. Read `README.md` for the overview and the subsystem notes in
`docs/` before changing an area they cover.

## What myos is

A small Rust kernel (`#![no_std]`) that boots through Limine in QEMU on
**x86_64** (BIOS and UEFI), **AArch64** and **RISC-V** (riscv64imac), with a
VFS, kernel modules, preemptive scheduling, signals, networking (smoltcp in
userspace) and a C/Rust userspace built on newlib + libgloss. Ported programs
(oksh, sbase, ubase, ripgrep, tcc, uutils coreutils, vim, git, curl,
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
| `packages/<name>/` | the same, for programs CI builds and publishes but the image does not carry (coreutils, vim, git, lynx, lua, make, os-test, x11-libs, tinyx, x11-xft, x11-fonts, dwm, st, dmenu, x11-apps, bottom, clear, ncurses; `get-myos` installs them, `docs/packages.md`); a port moves between the two by moving its directory |
| `linux-compat/` | optional Linux syscall layer userspace (launcher, musl build, tests, `get-alpine`) |
| `scripts/` | CI scripts, registry, `ports.sh` (reads the descriptors), thin wrappers for port builds |
| `targets/` | custom Rust target specs for userspace |
| `etc/` | `etc/policy`, the default security policy (`/etc/policy` in the image, and the kernel's fallback; `docs/security.md`) |
| `docs/` | design notes per subsystem (signals, sockets, Linux layer, security, ...) |
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
  `std` sysroot). Everything lands in `target/`. Pulling CI's cached ports
  instead is much faster: "Fast local setup" below.

## Testing

The real test is booting: `test-mini` / `test-full` boot headless, log in
and run a test list **in the guest** (the ports' `test.sh`, the kernel's
`user/tests/kernel.sh`),
while the host watches the console for `TEST <name> PASS|FAIL` lines
(`src/boot_test.rs`, `docs/testing.md`). QEMU runs under TCG, so a boot
takes minutes.

```sh
cargo run -- test-mini                     # quick list (what PR CI runs)
cargo run -- aarch64 test-mini             # also riscv64, uefi
cargo run -- test-full                     # full list: + packages, HTTPS, SSH, Alpine, curated os-test (must be 100%)
cargo run -- packages                      # the package tarballs + indexes (target/packages/)
scripts/local-ci.sh [bios|uefi|aarch64|riscv64] [mini|full]   # the same with OOM/TCG settings for a loaded host
cargo test -p ps2-scancode -p ext2fs -p fatvol   # host unit tests (ext2fs needs e2fsprogs, fatvol dosfstools and mtools)
cargo test --manifest-path target/smoltcp-myos/Cargo.toml --lib   # netd's patched smoltcp (after a build)
```

- Test on every arch you could have affected; arch-specific code needs all
  three. Don't over-test: a mini run per affected arch is usually enough
  locally, CI does the rest.
- New behavior gets a test: a function and a `t` line in the `test.sh` of
  the port or program it belongs to (`PORT_TEST` in its `port.env`; a
  package's test runs in the full mode after the install; `host.sh` with
  `PORT_HOST` when the test needs a peer on the host), the kernel's own in
  `user/tests/kernel.sh`, a smoke program under `user/c`, or an os-test in
  the curated lists
  (`packages/os-test/overlay/misc/*.tests`). A test's output is shown only
  when it fails: keep passing tests quiet.
- The full mode needs the network (`https://example.com/`, the Alpine
  mirror for `get-alpine`); the packages come from the build itself, served
  by the launcher to the guest (`docs/packages.md`). In a sandbox with a
  TLS-intercepting proxy, append its CA to `target/cacert.pem` for local
  runs only and restore it afterwards; never commit it.

## Fast local setup (fresh container, cloud agent)

Building every port from source takes hours; CI's port cache on GHCR gets a
fresh Ubuntu container to its first boot in about 10 minutes (pull ~1 min,
first `cargo build` ~4 min, a `test-mini` boot ~95 s on 4 cores). In order:

```sh
sudo apt install qemu-system-x86 qemu-system-arm qemu-system-misc \
  qemu-efi-aarch64 qemu-efi-riscv64 clang lld make git libc6-dev rsync \
  patch curl zstd e2fsprogs dosfstools mtools
export GITHUB_REPOSITORY=davide-matasci/myos
./scripts/ci-registry.sh pull all            # every port CI built for these sources
# What CI's build job runs next: each port's build script, which skips a
# current port but still installs what it stages (libtcc1.a into newlib).
./scripts/ports.sh --build-list all | while read -r _ s; do "./$s"; done
cargo build && cargo run -- uefi test-mini
```

What breaks in an agent sandbox (Claude Code on the web and the like):

- **Its `GITHUB_TOKEN` / `GH_TOKEN` is scoped to this repository.** GHCR
  refuses it (`registry miss ...: login failed`) and GitHub refuses it for
  the upstreams `scripts/git-retry.sh` clones ("could not read Username").
  The packages and upstreams are public: run the pull, the port loop and
  the cargo commands under `env -u GITHUB_TOKEN -u GH_TOKEN`.
- **GitHub `/archive/` tarballs of other repositories may get a 403**
  (release assets and git clones work). Only mbedtls is fetched that way;
  when `ports/mbedtls/fetch.sh` fails, lay the same tree out from its tag:
  ```sh
  . ports/mbedtls/versions.env
  git clone -q --depth 1 --branch "v$MBEDTLS_VERSION" https://github.com/Mbed-TLS/mbedtls target/mbedtls-src
  rm -rf target/mbedtls-src/{.git,programs,tests,docs}
  echo "$MBEDTLS_VERSION" > target/.mbedtls-src-version
  ```
- **The launcher's OVMF download fails behind a TLS-intercepting proxy**
  (`InvalidCertificate(UnknownIssuer)`: the `ovmf-prebuilt` crate has its own
  CA roots). curl it into the crate's cache; the tag and sha256 are
  `Source::LATEST` in `~/.cargo/registry/src/*/ovmf-prebuilt-*/src/source_constants.rs`:
  ```sh
  tag=edk2-stable202605-r1 sha=8ae4d2d73161cc2335f5675d3b8b6edfa0642301679764a246940488ea3ce20d
  curl -fsSL -o /tmp/ovmf.tar.xz "https://github.com/rust-osdev/ovmf-prebuilt/releases/download/$tag/$tag-bin.tar.xz"
  echo "$sha  /tmp/ovmf.tar.xz" | sha256sum -c - && mkdir -p target/ovmf \
    && tar -xJf /tmp/ovmf.tar.xz -C target/ovmf --strip-components=1 && printf %s "$sha" > target/ovmf/sha256
  ```
- Running without the token may need the user's permission in the agent's
  settings; ask rather than work around a refusal.

Intermittent failures (a race, a flaky boot) need a loop, not one CI run:
a race may fail a few boots in ten or none in twenty-five, so a green run
says little. Boot the unfixed base the same number of times first; a fix
is shown only when the base fails and the fix does not. Loading the host
CPUs (`sha256sum /dev/zero &` a few times) widens TCG's timing windows.
`scripts/local-ci.sh` defaults to single-threaded TCG (`MYOS_TCG_SINGLE=1`),
which hides SMP races: hunt those with `cargo run` directly.

```sh
for i in $(seq 10); do cargo run -q -- uefi test-mini > /tmp/boot$i.log 2>&1 \
  && rm /tmp/boot$i.log || echo "run $i failed"; done
```

## CI (GitHub Actions)

- `ci.yml` calls `ci-ports.yml` (cross-builds each port, cached as OCI
  artifacts on GHCR keyed by a hash of its inputs, `scripts/ci-registry.sh`;
  its plan job, `scripts/ci-ports-plan.sh`, starts a toolchain or port job
  only for what the registry lacks) and `ci-runtime.yml` (build job → boot
  jobs).
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

- **No ABI workarounds**: the kernel, the modules, libc and every port and
  package are rebuilt from this tree together, so an ABI (the native
  syscalls, the module `KernelApi` and its `#[repr(C)]` types in
  `modules/abi`, libgloss) is changed the right way: widen the field,
  change the struct or the call. Never add a compatibility shim to keep
  old binaries working (an appended `_hi` field, a second call beside the
  old one). Bump `ABI_VERSION` (`modules/abi`) when the module ABI changes.
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
- **Pending work is a GitHub issue**, not a TODO file or a list in a doc:
  a follow-up, a known bug or a limit worth lifting that a change leaves
  open gets an issue (what and why, where in the code, a `bug` or
  `enhancement` label, "**Low priority.**" first when it is). Search the
  open issues before opening one, and point to it as `issue #N` from code
  comments and docs.
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
- Copyleft components already ship in the images or as packages (git, lynx,
  GNU make, TinyCC, TinyX, the Mozilla CA bundle; see the notices file). Distributing their
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
