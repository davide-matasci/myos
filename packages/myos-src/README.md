# myos-src

The source the system was built from, as an app, with everything a build
of its kernel reads: myos builds its own kernel from it, with its own
compiler and no network.

```sh
mkfs.ext2 /dev/nvme1n1/data && mount /dev/nvme1n1/data /mnt ext2
get-myos -r /mnt/apps myos-src        # with rustc and llvm (PORT_RDEPS)
sh /mnt/apps/myos-src/src/myos/scripts/self-host.sh -r /mnt/apps
```

On a disk: the three apps are about 1 GiB, and the build writes a few
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

The crates are fetched from crates.io when the package is built, as in
any build of the tree; none is checked in. Each keeps its license files
(`THIRD_PARTY_NOTICES.md`).

The descriptor's kind for it is `source:` (`docs/ports.md`): a tree
packed with every file, where `tree:` leaves the dot files and the
`.txt` files out. cargo needs `.cargo/config.toml`, and each vendored
crate's `.cargo-checksum.json` names every file of the crate.

## What `scripts/self-host.sh` builds

`cargo build -p kernel` for the kernel's bare-metal target on the arch
it runs on, as the launcher does on a build machine: the kernel, and
through `kernel/build.rs` every module and every `no_std` program
(`init`, `netd`, ...), each at `target/<name>-<target>`.

- **The compiler** is the rustc package's (`packages/rustc`), which has
  `core` and `alloc` for the bare-metal targets of its arch. The build
  scripts and proc macros are programs of myos, linked against mlibc.
- **The links** of the bare-metal programs are lld's own
  (`CARGO_TARGET_<TARGET>_LINKER=ld.lld`): rustc's linker for a myos
  program adds the C library.
- **`RUSTC_BOOTSTRAP=1`**: the tree's cargo settings (an artifact
  dependency, a JSON target) are unstable ones.

## Limits

- `http` is not built: its TLS is LibreSSL over newlib, C that is
  cross-built on a build machine. The script takes the running system's
  (`MYOS_PREBUILT`, `kernel/build.rs`).
- The kernel alone: the C userspace (newlib, mlibc, the ports), the std
  programs and the image are built on a build machine.
- Not the launcher: `vendor/` holds its crates too, but it is a program
  for a Linux or macOS host.

`test.sh` builds a module on every arch and loads it; on x86_64, where
CI's machine runs on the host's CPU, the whole kernel.
