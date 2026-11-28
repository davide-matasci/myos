# myos-src

The source the system was built from, as an app, with everything a build
of its kernel and its Rust programs reads: myos builds them from it, with its own
compilers and no network.

```sh
mkfs.ext2 /dev/nvme1n1/data && mount /dev/nvme1n1/data /mnt ext2
get-myos -r /mnt/apps myos-src        # with rustc, llvm, make, awk and python (PORT_RDEPS)
sh /mnt/apps/myos-src/src/myos/scripts/self-host.sh -r /mnt/apps
```

On a disk: the apps are about 1 GiB, and the build writes a few
GiB under the tree's `target/`.

## What is in the app

`/src/myos`, the tree of the commit the package was built at:

| Path | What |
|------|------|
| the tree | every checked-in file (`git ls-files`) |
| `vendor/` | every crate of every workspace in the tree: the root's, each module's and each program's (`cargo vendor`) |
| `Cargo.lock` beside each workspace's manifest | the versions `vendor/` holds (the repository keeps no lock file) |
| `.cargo/config.toml` | the tree's, with crates.io replaced by `vendor/` |
| `target/acpi-myos`, `target/smoltcp-myos` | the two crates the tree patches, as their prepare scripts leave them, with the stamps that tell those scripts there is nothing to fetch |
| `target/newlib-src` | newlib's source, the C library of the C userspace, as `toolchain/newlib/fetch.sh` and `patch.sh` leave it (the pinned release with libgloss/myos in it), without its history |
| `target/libressl-src` | LibreSSL's source, which `http` links (`user/tls`), as `ports/libressl/fetch.sh` leaves it: the pinned release with the myos patch, and its stamp |
| `target/rust-std-patch` | Rust's library with myos in it, which the `std` of the std programs is built from: the pinned nightly's `rust-src` as `toolchain/std/patches/wire-myos.py` patches it, with the crates it vendors |

The crates are fetched from crates.io when the package is built,
newlib from its repository, LibreSSL and the ports from their mirrors and the library from the toolchain rustup
installed, as in any build of the tree; none is checked in. Each keeps its license files (`THIRD_PARTY_NOTICES.md`).

The descriptor's kind for it is `source:` (`docs/ports.md`): a tree
packed with every file, where `tree:` leaves the dot files and the
`.txt` files out. cargo needs `.cargo/config.toml`, and each vendored
crate's `.cargo-checksum.json` names every file of the crate.

## What `scripts/self-host.sh` builds

First newlib and libgloss, for the arch it runs on
(`toolchain/newlib/build.sh`): newlib's `configure` and `make` with the
llvm app's clang, then libgloss/myos and the C both libcs share, to
`target/newlib-<arch>`. The make, awk and python apps are for this step.

Then the C programs against it (`scripts/build-c-smokes.sh`: `user/c`,
`/bin/etc/sec`, `play` and `battery` among them).

Then LibreSSL's three libraries, which `http` links: the port's build
script (`ports/libressl/build.sh`) with clang, against that newlib.

Then the ports of the image that are C against newlib, each by its build
script: oksh (`/bin/sh`), sbase, ubase, dropbear, curl, Limine's tool, and
`get-myos` with zlib. Their sources are in the package
(`target/<port>-myos-build`, `target/curl-src`, `target/zlib-src`,
`target/limine-src`), fetched and patched where it was built.

Then `std` for `<arch>-unknown-myos`, from `target/rust-std-patch`
(`toolchain/std/build-sysroot.sh`, `cargo -Z build-std`, with the crates
the library vendors in place of crates.io), and the std programs over it
(`user/std/build.sh`: `target/std-<name>-<arch>-unknown-myos`). The
sysroot they link, `target/myos-sysroot`, has the rustc app's own
libraries by a symbolic link: what build scripts are compiled against.

Then `cargo build -p kernel` for the kernel's bare-metal target, as the
launcher does on a build machine: the kernel, and through
`kernel/build.rs` every module and every `no_std` program (`init`,
`netd`, `http`, ...), each at `target/<name>-<target>`.

- **The compiler** is the rustc package's (`packages/rustc`), which has
  `core` and `alloc` for the bare-metal targets of its arch. The build
  scripts and proc macros are programs of myos, linked against mlibc.
- **The links** of the bare-metal programs are lld's own
  (`CARGO_TARGET_<TARGET>_LINKER=ld.lld`): rustc's linker for a myos
  program adds the C library.
- **`RUSTC_BOOTSTRAP=1`**: the tree's cargo settings (an artifact
  dependency, a JSON target) are unstable ones.

Then the initramfs, `target/initramfs-<arch>.cpio.zst`: the running
system's (`/proc/boot/initramfs`) with every module and program built
here in place of its own, packed by `user/mkinitramfs`, which the script
builds first (a program of myos, for the arch it runs on). With the
kernel it is a boot slot (`docs/install.md`):

```sh
cp target/x86_64-unknown-none/debug/kernel /boot/boot/a/kernel
cp target/initramfs-x86_64.cpio.zst /boot/boot/a/initramfs
```

## Limits

- mlibc, what links it and the ports are
  built on a build machine: the new initramfs has the running system's.
  So is the boot disk (the ESP with Limine).
- Not the launcher: `vendor/` holds its crates too, but it is a program
  for a Linux or macOS host.

`test.sh` builds a module on every arch and loads it; on x86_64, where
CI's machine runs on the host's CPU, the kernel and the initramfs, which
the next boot then runs, fetching a page over TLS with its `http`.
