# Ports and packages: the descriptors

Every program the images carry, and every package that is built but not
shipped in them, has one `port.env` file in its directory: its
descriptor. The build, the initramfs packer, the CI registry and the CI
matrices all read these descriptors; there is no other list of ports and
no Cargo feature per port. Read this page when you add a port, move one
between the image and the packages, or change how one is built.

## Roles: where a directory lives

| Directory | Role |
|-----------|------|
| `ports/<name>/` | in every image (initramfs) |
| `user/<name>/`, `user/c/`, `user/std/` | the native userspace, in the image |
| `toolchain/newlib/`, `toolchain/std/`, `toolchain/mlibc/`, `toolchain/libcxx/` | the toolchains (`docs/mlibc.md`); nothing of them is in the image |
| `packages/<name>/` | built and cached by CI, installed on a running system with `get-myos` (`docs/packages.md`) |

The role is the directory. **Moving `ports/foo` to `packages/foo` takes foo
out of the image, and moving it back puts it in**, with no other change.
CI builds and caches both roles the same way, and runs a job only for a
port the registry lacks (`scripts/ci-ports-plan.sh`).

## Adding a port

1. Create `ports/<name>/` (or `packages/<name>/`) with `versions.env`
   (pin + sha256), `fetch.sh`, a `build.sh` that writes
   `target/.myos-<name>-version` with the hash when done, patches as
   `*.myos.patch`, a `README.md`, and `port.env`. `ports/lua` is a good
   model.
2. Add `myos_<name>_version_hash` / `myos_<name>_is_current` to
   `scripts/myos-c-userspace-lib.sh` (copy an existing pair; dashes become
   underscores). The first hashes the port's inputs, the second checks
   that its outputs exist and match.
3. Add a boot test: `test.sh` in the port directory with
   `PORT_TEST=test.sh` (`docs/testing.md`), and `host.sh` with
   `PORT_HOST=host.sh` when the test needs a peer on the host. A
   package's test runs in the full mode, after the install.
4. Update `THIRD_PARTY_NOTICES.md` (see "License compliance" in
   `AGENTS.md`).

Nothing else changes: everything else is descriptor-driven.

### Patches and compat files

Patches (`*.myos.patch`) and compat files (`myos_compat.h`,
`myos_stubs.c`) are for what myos genuinely lacks: a kernel feature, a
declaration newlib's headers leave out, build glue. A function several
ports would stub belongs in the libc instead: in `toolchain/libmyos`
(`docs/mlibc.md`) when it is the kernel's interface, in libgloss
(`toolchain/newlib/libgloss/myos`) when it is newlib's alone. A
workaround for a gap that has since been filled is removed.

## The descriptor

`port.env` is sourced by the shell (`scripts/ports.sh`) and parsed line
by line in Rust (`src/ports.rs`). Its lines are `KEY=value` or
`KEY="value ..."`; `#` starts a comment. Every key has a default, so a
minimal port needs only `PORT_FILES`.

| Key | Default | Meaning |
|-----|---------|---------|
| `PORT_NAME` | the directory name | registry, stamp and matrix name |
| `PORT_KIND` | `port` | `port` (cross-built by a script), `user` (a Rust crate built by `kernel/build.rs`), `c` (the C smokes), `std` (the `std` demos), `toolchain` (newlib, the sysroot: built first, by CI jobs of their own) |
| `PORT_LIBC` | `newlib` | the C library to build against: `newlib` or `mlibc` (`docs/mlibc.md`) |
| `PORT_CORE` | `0` | `1` for what a boot needs |
| `PORT_DEPS` | | ports to build first (`vim` needs `ncurses`); build order only |
| `PORT_RDEPS` | | packages this one needs on the running system, which `get-myos` installs first (`docs/packages.md`) |
| `PORT_BUILD` | | the build script: a name in the port directory, or a repo-relative path with `/`. Empty: nothing to build |
| `PORT_STAMP` | `.myos-<name>-version` | the `target/` file the script writes its input hash to when done |
| `PORT_ARCH_JOBS` | `0` | `1` for a port too long to build in one CI job (below) |
| `PORT_TIMEOUT` | `45` | the CI ports job's limit for the port, in minutes; at most 360 |
| `PORT_OUTPUTS` | | what the script leaves under `target/` (what the registry caches); globs are allowed |
| `PORT_READY` | the first file output | a `target/` file whose presence means the port was built |
| `PORT_FILES` | | what the image (or the package) gets, below |
| `PORT_TEST_FILES` | | what the tests module gets instead (`docs/testing.md`), same syntax |
| `PORT_BIN`, `PORT_WATCH`, `PORT_PREPARE` | | `user` only: the binary name, extra watched sources, a script run before the build (netd's patched smoltcp) |
| `PORT_TEST` | | the port's boot test script (`docs/testing.md`) |
| `PORT_HOST` | | the host's side of that test, run by the launcher on a `HOST <name> <args>` line |

`src/ports.rs` gives the finer points of each key.

Paths in `PORT_OUTPUTS`, `PORT_READY` and `PORT_FILES` are relative to
`target/` and expand per arch:

| Placeholder | x86_64 | aarch64 | riscv64 |
|-------------|--------|---------|---------|
| `{arch}` | `x86_64` | `aarch64` | `riscv64` |
| `{none}` | `x86_64-unknown-none` | `aarch64-unknown-none` | `riscv64-unknown-none` |
| `{myos}` | `x86_64-unknown-myos` | `aarch64-unknown-myos` | `riscv64-unknown-myos` |
| `{kernel}` | `x86_64-unknown-none` | `aarch64-unknown-none-softfloat` | `riscv64imac-unknown-none-elf` |

### `PORT_FILES`

Space-separated entries, `kind:source:destination`, the destination a
path in the image:

| Entry | Puts in the image |
|-------|-------------------|
| `bin:<target file>:<path>[,<alias>...]` | one ELF, mode 0755; the aliases are hard links (`oksh-{none}:bin/custom/sh,bin/sh`) |
| `data:<target file>:<path>` | a file the build produced (`cacert.pem:lib/cacert.pem`) |
| `file:<port dir file>:<path>` | a checked-in file of the port directory (`vimrc:lib/vim/vimrc`) |
| `manifest:<target file>:<dir>` | every `name:/path/to/elf` line of the manifest as `<dir>/<name>` (ubase) |
| `multicall:<elf>:<manifest>:<dir>` | the ELF once, every name of the manifest hard-linked to it (sbase, uutils coreutils) |
| `tree:<target dir>:<dir>` | a directory tree, keeping the host exec bits |

A missing source file fails the build: the packer never silently leaves a
file out. The kernel embeds no program; every one, `init` included,
comes from the initramfs.

### Ports built an arch at a time

A port with `PORT_ARCH_JOBS=1` (`webkitgtk`) is built by CI as
`<name>:<arch>`, one job per arch, with `MYOS_<NAME>_ARCHES` set to that
arch (the name in capitals, `-` as `_`). Its script builds the arches
the variable lists (all by default) and writes a stamp per arch. The
registry caches each arch apart; `ci-registry.sh pull <name>` pulls the
three.

## What reads the descriptors

`build.rs` runs the build script of every image port whose `PORT_READY`
is missing, `kernel/build.rs` builds the `user` ports, and
`src/initramfs.rs` packs the `PORT_FILES`. On the shell side,
`scripts/ports.sh` (`--list`, `--build-list`, `--matrix`, ..., or
`myos_port_load NAME` as a library) serves the CI registry, the CI jobs
and the `ci-ports.yml` matrix.

## The userspace ISA

Ports compile for the arch's bare target (`<arch>-unknown-none`) through
newlib's tool wrappers, `myos_write_cross_cc` or `myos_mlibc_cflags`,
which add the userspace's ISA (`myos_arch_cflags` in
`scripts/myos-c-userspace-lib.sh`):

| Arch | ISA | Float ABI |
|------|-----|-----------|
| x86_64 | clang's default | hard float, SSE2 |
| aarch64 | clang's default | hard float, NEON |
| riscv64 | `-march=rv64gc -mabi=lp64d` (`MYOS_RISCV64_ISA`) | hard float, as on Linux |

clang's default for a bare riscv64 triple is soft float, so a compile
that bypasses the wrappers fails to link with the rest. The `std`
programs' Rust targets (`targets/*-unknown-myos.json`) and the `no_std`
programs' hard-float bare targets (`kernel/build.rs`) match. The kernel
and its modules stay soft-float (`aarch64-unknown-none-softfloat`,
`riscv64imac-unknown-none-elf`): they never touch the FP registers, and
the kernel switches every user task's.
