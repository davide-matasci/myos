# rust

Rust for myos programs, built in myos: `get-myos rust`, then

```sh
insmod /lib/modules/linux                 # the Linux layer (an image with linux_compat)
mkfs.ext2 /dev/nvme1n1/data && mount /dev/nvme1n1/data /mnt ext2   # room: about 1 GiB
run-myos rust -r /mnt/rust setup          # once, and after an upgrade of the app
mkdir -p /mnt/rust/src && cd /mnt/rust/src
run-myos rust -r /mnt/rust cargo new --vcs none hello && cd hello
run-myos rust -r /mnt/rust cargo build --release
./target/x86_64-unknown-myos/release/hello
```

`-r ROOT` (or `RUST_ROOT`) names the toolchain's root, `/data/rust`
without either.

## How

The compiler is not ported: `rust setup` runs the official build of the
nightly `rust-toolchain.toml` pins for the host's musl
(`x86_64-unknown-linux-musl`, `aarch64-unknown-linux-musl`,
`riscv64gc-unknown-linux-musl`) under the Linux layer
(`docs/linux-compat.md`), in an Alpine root it makes with `get-alpine -n`
(busybox, `libgcc` and `libatomic` for the `libgcc_s` of rustc and
`rust-lld` and riscv64's `libatomic`, curl and xz for the download;
unpacked natively, without apk running under emulation). It fetches rustc, cargo and the host's std from
static.rust-lang.org at run time, the server checked against the image's
CA bundle and each tarball against the sha256 `versions.env` pins
(`pin.sh DATE` writes them from that nightly's channel manifest;
`toolchain/std/bump-nightly.sh --write` runs it), and installs them in the
root's `/usr/local`. Nothing of the toolchain is in the image or the
package.

It is that nightly because the std is: the app carries the myos std of
its arch's target (`/lib/rust`: the rlibs of `target/myos-sysroot`, the
one the ports are built with, and the target spec), and an rlib loads only
in the rustc that built it. `setup` copies it into the root's sysroot.
The target is named by its spec's path, as the sysroot was built (the
rlibs carry a hash of the spec); the root's cargo configuration
(`/root/.cargo/config.toml`, written by `setup`) makes it the default
target, so `cargo build` builds for myos, into `target/<arch>-unknown-myos/`.

Both links are the toolchain's own `rust-lld`'s, a program that is not
position-independent: the Linux layer loads it at its link address. A
myos program links as on the host, the target spec naming `rust-lld`.
Build scripts and proc macros are built for the host and run under the
layer (no C compiler in the root), both linked dynamically against the
root's musl and `libgcc_s`: a proc macro is a shared object rustc loads,
and a build script would gain nothing from linking statically. A linker
wrapper in
`/usr/local/libexec/myos` names musl's loader and points lld at those two
libraries alone; cargo's `[host]` table (unstable) gives the host's
programs their linker and `-crt-static`.

A command runs from a directory inside the root: the Linux program sees
the root only. What it builds is a native myos program, run as any other.

## Limits

- The std is the arch's own: a toolchain on x86_64 builds for
  `x86_64-unknown-myos`, not for the other two arches.
- Crates that need a C compiler in their build scripts (`cc`) do not build:
  the root has none.
- The download is about 230 MB and the root about 1 GiB (rustc's LLVM,
  and `rust-lld`); under emulation the setup takes minutes and a first
  build too.

`test.sh` sets the toolchain up on the scratch disk and builds a program
with a build script and a proc macro of its own, then runs it.
