# Ports and packages: the descriptors

Every program the images carry, and every package that is built but not
shipped in them, is described by one `port.env` file in its directory. The
build (`build.rs`, `kernel/build.rs`), the initramfs packer
(`src/initramfs.rs`), the CI registry and the CI matrices
(`scripts/ports.sh`) read these descriptors: there is no other list of
ports, and no Cargo feature per port.

## Roles: where a directory lives

| Directory | Role | Shipped |
|-----------|------|---------|
| `ports/<name>/` | image | in every image (initramfs) |
| `user/<name>/`, `user/c/`, `user/std/` | image | the native userspace (Rust programs, C smokes, the `std` demos) |
| `toolchain/newlib/`, `toolchain/std/` | image | the toolchains: newlib (its sysroot tree is in the image, for tcc) and the Rust `std` sysroot (nothing in the image) |
| `packages/<name>/` | package | built and cached by CI, not in the image; installed on a running system with `get-myos` (`docs/packages.md`) |

The role is the directory: **moving `ports/foo` to `packages/foo` takes foo
out of the image, moving it back puts it in**, with no other change. CI
builds and caches both roles the same way (`ci-ports.yml` reads its matrices
from `scripts/ports.sh --matrix`, and runs a job only for a port the
registry lacks, `scripts/ci-ports-plan.sh`).

## The descriptor

`port.env` is sourced by the shell and parsed line by line in Rust
(`src/ports.rs`): `KEY=value` or `KEY="value ..."`, `#` comments. Every key
has a default, so a minimal port needs only `PORT_FILES`.

| Key | Default | Meaning |
|-----|---------|---------|
| `PORT_NAME` | the directory name | registry package, stamp and matrix name (`user/c` is `c-smokes`, `user/std` is `std-hello`, `toolchain/std` is `sysroot`) |
| `PORT_KIND` | `port` | `port` (cross-built by a script), `user` (a Rust crate built by `kernel/build.rs`), `c` (the C smokes), `std` (the `std` demos), `toolchain` (newlib, the sysroot: built before everything else, by their own CI jobs; `build.rs` leaves them to the ports' scripts) |
| `PORT_CORE` | `0` | `1` for what a boot needs (the shell, getty, init's helpers, the CI smokes) |
| `PORT_DEPS` | | ports to build first (`vim` needs `ncurses`, the Rust ports need `sysroot`): the build loops order by them, and the CI ports job builds a dependency itself when the registry does not have it (`sysroot` means: wait for the sysroot job's artifact). Build order only: a runtime need is `PORT_RDEPS` |
| `PORT_RDEPS` | | packages this one needs on the running system (`st` needs `x11-xft` and `x11-fonts`, `x11-xft` needs `x11-libs`): the `deps` field of the package index, which `get-myos` installs first (`docs/packages.md`). Only a package with files can be one (an image port is always there); `cargo run -- packages` refuses an unknown name or a loop |
| `PORT_BUILD` | | the build script: a name is in the port directory (`build.sh`), a path with `/` is repo-relative (`scripts/build-c-smokes.sh`). Empty: nothing to build (`ports/termcap` ships a checked-in file) |
| `PORT_STAMP` | `.myos-<name>-version` | the `target/` file the script writes its input hash to when done (the sysroot's is inside the sysroot) |
| `PORT_OUTPUTS` | | what the script leaves under `target/`, for the three arches (what the CI registry caches, with the stamp). Globs are allowed (`sbase-*-{none}`) |
| `PORT_READY` | the first file output | a `target/` file whose presence means the port was built (`build.rs` runs the script when it is missing). Needed when the first output is a directory |
| `PORT_FILES` | | what the image (or the package) gets, see below |
| `PORT_BIN` | | `user` only: the crate's binary name (`myos_cat`) |
| `PORT_IMAGE_BASE` | `0` | `user` only: `1` links the program at `USER_BASE` as `ET_EXEC` on aarch64 and riscv64 (programs with absolute vtables: netd, ping, http, dns) |
| `PORT_WATCH` | | `user` only: extra source files the kernel build watches, relative to the crate (`../lib/src/lib.rs`) |
| `PORT_PREPARE` | | `user` only: a script `kernel/build.rs` runs before it builds the crate, named like `PORT_BUILD`: what the build needs in `target/` first. netd's fetches smoltcp and applies myos's patches to it (`user/net/smoltcp/prepare.sh`, its `[patch.crates-io]`) |
| `PORT_TEST` | | the port's boot test script, in the port directory (`test.sh`): packed as `lib/myos-tests/ports/<group>-<name>.sh` (`0` core image port, `1` image port, `2` package), run by the test runner in that order (`docs/testing.md`) |
| `PORT_HOST` | | the host's side of that test, in the port directory (`host.sh`): not packed; the launcher runs it with the arguments of a `HOST <name> <args>` line the guest test prints (dropbear's SSH clients, the listen test's peer) |

Paths in `PORT_OUTPUTS`, `PORT_READY` and `PORT_FILES` are relative to
`target/` and expand per arch: `{arch}` (`x86_64`), `{none}`
(`x86_64-unknown-none`), `{myos}` (`x86_64-unknown-myos`), `{kernel}`
(`x86_64-unknown-none`, `aarch64-unknown-none-softfloat`,
`riscv64imac-unknown-none-elf`).

### `PORT_FILES`

Space-separated entries, `kind:source:destination`. The destination is a
path in the image (the initramfs root is `/`).

| Entry | Puts in the image |
|-------|-------------------|
| `bin:<target file>:<path>[,<alias>...]` | one ELF, mode 0755; the aliases are hard links (`oksh-{none}:bin/custom/sh,bin/sh`) |
| `data:<target file>:<path>` | a file the build produced (`cacert.pem:lib/cacert.pem`) |
| `file:<port dir file>:<path>` | a checked-in file of the port directory (`vimrc:lib/vim/vimrc`); `build.rs` watches it |
| `manifest:<target file>:<dir>` | every `name:/path/to/elf` line of the manifest as `<dir>/<name>` (sbase, ubase) |
| `multicall:<elf>:<manifest>:<dir>` | the ELF once, every name of the manifest hard-linked to it under `<dir>` (uutils coreutils) |
| `tree:<target dir>:<dir>` | a directory tree (os-test sources and prebuilt tests), keeping the host exec bits |

A missing source file fails the build: the packer never silently leaves a
file out. The kernel embeds no program: every one, `init` included, comes
from the initramfs.

## What reads the descriptors

- `build.rs`: for every image port with a build script, runs the script
  when `PORT_READY` is missing (the scripts skip themselves when current),
  and watches `port.env`, the `file:` sources and the test script.
- `kernel/build.rs`: builds every `user` port for the kernel's arch into
  `target/<bin>-<triple>`, where the initramfs packer takes it.
- `src/initramfs.rs`: packs `PORT_FILES` of every image port, for the arch
  being imaged.
- `scripts/ports.sh`: the shell side. `--list`, `--outputs NAME`,
  `--image-files NAME`, `--all-files`, `--all-outputs`, `--stamps`, `--build-list`,
  `--matrix`, `--tests`; as a library (`myos_port_load NAME`) for
  `scripts/ci-registry.sh` (what to cache, under the port's name), the CI
  build job (`ci-build-kernels.sh` builds every image port before hashing
  the kernel inputs; the port stamps are part of that hash),
  `ci-pack-build-artifacts.sh` and `ci-assert-boot-artifacts.sh` (what a
  boot job needs) and the `ci-ports.yml` ports matrix.
- The registry needs `myos_<name>_version_hash` and `myos_<name>_is_current`
  in `scripts/myos-c-userspace-lib.sh` (dashes as underscores; the sysroot's
  are in `toolchain/std/lib.sh`): the content hash of the port's inputs, and
  the check that its outputs exist and match.
- The two toolchains are ports too (`toolchain/newlib`, `toolchain/std`),
  so the registry, the stamps and the image files treat them like the rest;
  only their CI jobs are their own (`sysroot`, `newlib` in `ci-ports.yml`:
  every port pulls them, and the sysroot build is long and cached apart).

## Adding a port

1. `ports/<name>/` (or `packages/<name>/`): `versions.env` (pin + sha256),
   `fetch.sh`, `build.sh` writing `target/.myos-<name>-version` with the
   hash when done, patches as `*.myos.patch`, a `README.md`, and `port.env`.
2. `myos_<name>_version_hash` / `myos_<name>_is_current` in
   `scripts/myos-c-userspace-lib.sh` (copy an existing pair).
3. A boot test: `test.sh` in the port directory with `PORT_TEST=test.sh`
   (and `host.sh` with `PORT_HOST=host.sh` when it needs a peer on the host)
   in the descriptor (`docs/testing.md`; a package's test runs in the full
   mode, after the install).
4. `THIRD_PARTY_NOTICES.md` (see "License compliance" in `AGENTS.md`).

Patches (`*.myos.patch`) and compat files (`myos_compat.h`, `myos_stubs.c`)
are for what myos genuinely lacks: a kernel feature (job control, timer
signals, rlimits, `/var`), a declaration newlib's headers leave out, or
build glue. A function several ports would stub belongs in libgloss
(`toolchain/newlib/libgloss/myos`), implemented over the existing
primitives; a workaround for a gap that has since been filled is removed
(the audit that trimmed them is in the history of this file's ports).

Nothing else: `Cargo.toml`, the workflows, the registry, the pack lists and
the initramfs packer are descriptor-driven.
