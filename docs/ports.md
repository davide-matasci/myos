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
| `user/<name>/`, `user/c/` | image | the native userspace (Rust programs, C smokes) |
| `toolchain/std/` | image | the Rust `std` test programs (`/bin/std`) |
| `packages/<name>/` | package | built and cached by CI, not in the image |

The role is the directory: **moving `ports/foo` to `packages/foo` takes foo
out of the image, moving it back puts it in**, with no other change. CI
builds and caches both roles the same way (`ci-ports.yml` reads its matrices
from `scripts/ports.sh --matrix`).

## The descriptor

`port.env` is sourced by the shell and parsed line by line in Rust
(`src/ports.rs`): `KEY=value` or `KEY="value ..."`, `#` comments. Every key
has a default, so a minimal port needs only `PORT_FILES`.

| Key | Default | Meaning |
|-----|---------|---------|
| `PORT_NAME` | the directory name | registry package, stamp and matrix name (`user/c` is `c-smokes`, `toolchain/std` is `std-hello`) |
| `PORT_KIND` | `port` | `port` (cross-built by a script), `user` (a Rust crate built by `kernel/build.rs`), `c` (the C smokes), `std` (the `std` programs) |
| `PORT_CORE` | `0` | `1` for what a boot needs (the shell, getty, init's helpers, the CI smokes) |
| `PORT_DEPS` | | ports to build first (`vim` needs `ncurses`); CI builds these in `ports-advanced`, after `ports-base` |
| `PORT_SYSROOT` | `0` | `1` when the build needs the Rust `std` sysroot (`target/myos-sysroot`) |
| `PORT_BUILD` | | the build script: a name is in the port directory (`build.sh`), a path with `/` is repo-relative (`scripts/build-c-smokes.sh`). Empty: nothing to build (`ports/termcap` ships a checked-in file) |
| `PORT_OUTPUTS` | | what the script leaves under `target/`, for the three arches (what the CI registry caches, with the stamp). Globs are allowed (`sbase-*-{none}`) |
| `PORT_READY` | the first file output | a `target/` file whose presence means the port was built (`build.rs` runs the script when it is missing). Needed when the first output is a directory |
| `PORT_FILES` | | what the image (or the package) gets, see below |
| `PORT_BIN` | | `user` only: the crate's binary name (`myos_cat`) |
| `PORT_EMBED` | | `user` only: the kernel embeds the program at this binfs path (`custom/cat`), so a boot works without the initramfs |
| `PORT_IMAGE_BASE` | `0` | `user` only: `1` links the program at `USER_BASE` as `ET_EXEC` on aarch64 and riscv64 (programs with absolute vtables: netd, ping, http, dns) |
| `PORT_WATCH` | | `user` only: extra source files the kernel build watches, relative to the crate (`../lib/src/lib.rs`) |

Paths in `PORT_OUTPUTS`, `PORT_READY` and `PORT_FILES` are relative to
`target/` and expand per arch: `{arch}` (`x86_64`), `{none}`
(`x86_64-unknown-none`), `{myos}` (`x86_64-unknown-myos`), `{kernel}`
(`x86_64-unknown-none`, `aarch64-unknown-none-softfloat`,
`riscv64imac-unknown-none-elf`).

### `PORT_FILES`

Space-separated entries, `kind:source:destination`. The destination is a
path in the image (the initramfs root; `/bin/...` are the binfs trees).

| Entry | Puts in the image |
|-------|-------------------|
| `bin:<target file>:<path>[,<alias>...]` | one ELF, mode 0755; the aliases are hard links (`oksh-{none}:bin/custom/sh,bin/sh`) |
| `data:<target file>:<path>` | a file the build produced (`cacert.pem:lib/cacert.pem`) |
| `file:<port dir file>:<path>` | a checked-in file of the port directory (`vimrc:lib/vim/vimrc`); `build.rs` watches it |
| `manifest:<target file>:<dir>` | every `name:/path/to/elf` line of the manifest as `<dir>/<name>` (sbase, ubase) |
| `multicall:<elf>:<manifest>:<dir>` | the ELF once, every name of the manifest hard-linked to it under `<dir>` (uutils coreutils) |
| `tree:<target dir>:<dir>` | a directory tree (os-test sources and prebuilt tests), keeping the host exec bits |

A missing source file fails the build: the packer never silently leaves a
file out. The one exception is a `user` program with `PORT_EMBED`: the kernel
serves it from binfs, so its file in the initramfs is optional.

## What reads the descriptors

- `build.rs`: for every image port with a build script, runs the script
  when `PORT_READY` is missing (the scripts skip themselves when current),
  watches `port.env` and the `file:` sources, and tells the host tool which
  ports are in (`MYOS_IMAGE_PORTS`, for the `wait_ci` needles of tcc, git
  and dropbear).
- `kernel/build.rs`: builds every `user` port for the kernel's arch and
  generates the binfs registrations of the embedded ones.
- `src/initramfs.rs`: packs `PORT_FILES` of every image port, for the arch
  being imaged.
- `scripts/ports.sh`: the shell side. `--list`, `--outputs NAME`,
  `--image-files NAME`, `--all-image-files`, `--stamps`, `--image-list`,
  `--matrix base|advanced`; as a library (`myos_port_load NAME`) for
  `scripts/ci-registry.sh` (what to cache, under the port's name), the CI
  build job (`ci-build-kernels.sh` builds every image port before hashing
  the kernel inputs; the port stamps are part of that hash),
  `ci-pack-build-artifacts.sh` and `ci-assert-boot-artifacts.sh` (what a
  boot job needs) and the `ci-ports.yml` matrices.
- The registry needs `myos_<name>_version_hash` and `myos_<name>_is_current`
  in `scripts/myos-c-userspace-lib.sh` (dashes as underscores): the content
  hash of the port's inputs, and the check that its outputs exist and match.

## Adding a port

1. `ports/<name>/` (or `packages/<name>/`): `versions.env` (pin + sha256),
   `fetch.sh`, `build.sh` writing `target/.myos-<name>-version` with the
   hash when done, patches as `*.myos.patch`, a `README.md`, and `port.env`.
2. `myos_<name>_version_hash` / `myos_<name>_is_current` in
   `scripts/myos-c-userspace-lib.sh` (copy an existing pair).
3. A CI check: a command and needle in `src/wait_ci.rs`, gated on
   `port_enabled("<name>")` when the port is not core.
4. `THIRD_PARTY_NOTICES.md` (see "License compliance" in `AGENTS.md`), and
   `.github/path-filters.yml` only when the port's inputs live outside
   `ports/`, `packages/` and `user/c`.

Nothing else: `Cargo.toml`, the workflows, the registry, the pack lists and
the initramfs packer are descriptor-driven.
