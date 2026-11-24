# rustc

The Rust compiler and cargo, running on myos: `get-myos rustc`, then

```sh
run-myos rustc -O hello.rs && ./hello      # the compiler
mkdir hello && cd hello
run-myos rustc:cargo init --vcs none       # cargo
run-myos rustc:cargo build --release
./target/release/hello
```

No Linux layer and no download at run time: the compiler is a myos
program. It builds for its own target, `<arch>-unknown-myos-mlibc` (the
unix flavour of the std, `docs/userspace.md` "`std` on mlibc"), build
scripts and proc macros included: a build script is a program of the
same target, run by cargo, and a proc macro a shared object the compiler
loads.

## What is in the app

| Path | What |
|------|------|
| `/bin/custom/rustc` | the compiler's program, a few KiB over the library below |
| `/bin/custom/cargo` | cargo, static, with curl, libgit2 and libssh2 in it (TLS by LibreSSL) |
| `/lib/librustc_driver-*.so` | the compiler, with the llvm package's LLVM in it (X86, AArch64 and RISC-V) |
| `/lib/rustlib/<triple>/lib` | the std of the target: `core`, `alloc`, `std`, `proc_macro`, `test` |
| `/lib/rustlib/<triple>/bin/rust-lld` | the linker rustc runs: a script over the llvm app's `ld.lld` |
| `/lib/rustlib/<bare target>/lib` | `core`, `alloc` and `compiler_builtins` for the arch's bare-metal targets: the kernel's (`x86_64-unknown-none`, `aarch64-unknown-none-softfloat`, `riscv64imac-unknown-none-elf`) and, where it is another, the `no_std` programs' (`aarch64-unknown-none`, `riscv64gc-unknown-none-elf`) |

The app needs **llvm** (`PORT_RDEPS`): its `ld.lld` links, and its
`/lib/mlibc/lib` is the C library, libunwind and the compiler builtins a
myos program is linked against. `get-myos` installs it with rustc.

The compiler finds its sysroot from where its library is (`/lib`), not
from its program's directory.

## Where to install it

The compiler's library is about 145 MiB, more than a tmpfs file may be
(128 MiB, `docs/fs.md`), so the app goes on a disk: an ext2 partition
with a few hundred MiB, `get-myos -r /mnt/apps rustc` and
`run-myos -r /mnt/apps rustc ...`. cargo's `target/` belongs on a disk
too.

## How it is built

`build.sh`, an arch a CI job (`PORT_ARCH_JOBS`):

- The source is the tarball of the nightly `rust-toolchain.toml` pins
  (`versions.env`, `fetch.sh`): the compiler, cargo and every crate they
  need (`vendor/`).
- myos in it:
  - `rust-myos.myos.patch`: the three targets as built-in ones with myos
    as a host, and what the build needs for them;
  - the std's wiring on `library/` (`toolchain/std/patches/wire-myos.py`,
    the same as the sysroot's);
  - `port-crates.py` on the vendored crates: the `libc` crate gets the
    std's `libc-myos.patch`, and the few crates that list the systems
    they know (`getrandom`, `rustix`, `errno`, `libloading`, `socket2`)
    get `target_os = "myos"` where it belongs.
- The compiler's own build system then makes a compiler for the build
  machine from that source (stage 1, the first one that knows the myos
  targets), the std for myos with it, and the compiler and cargo for
  myos (stage 2) linked against that std.
- LLVM for myos is the `llvm` package's, the one clang and lld there are
  made of: upstream LLVM of the major this compiler is written for, not
  the fork the source carries. That port keeps LLVM's libraries and
  headers beside its program (`target/llvm-<arch>/dev`, not in its app),
  with a `llvm-config` that describes them to this build on the build
  machine. One LLVM in a system with both packages, built once; it is
  built for size, as clang is.
- The build downloads two things besides the source, both pinned by the
  source itself: the beta compiler it bootstraps from (`src/stage0`) and
  the LLVM Rust's CI built for this commit, for stage 1. Neither is in
  the package.

`files/rust-lld` is the link on myos, and `build.sh` writes the same for
the build machine: a static PIE by default (`--no-dynamic-linker`, as
clang's driver on myos), the shared mlibc and `/lib/ld.so` for a shared
object or a program over shared objects.

## Limits

- One target with a std: `<arch>-unknown-myos-mlibc`. The other flavour
  of the std (`<arch>-unknown-myos`, the Rust ports of the image) and the
  other arches are built on Linux, as before. The bare-metal targets of
  its own arch have `core` and `alloc`, as `rustup target add` gives a
  host: the kernel is built with them (`packages/myos-src`). Their links
  are `ld.lld`'s own (`CARGO_TARGET_<TARGET>_LINKER=ld.lld`): the
  `rust-lld` here is the link of a myos program.
- No Ctrl-C handler in the compiler (its crate goes through `nix`, which
  does not know myos): an interrupt ends it by the signal's default.
- cargo works offline and with path dependencies in the tests; fetching
  from crates.io needs the network and a CA bundle and is not tested.
- No rustdoc, clippy, rustfmt or rust-analyzer.

`test.sh` installs the app on the scratch disk, compiles and runs a
program with rustc, and builds one with cargo that has a build script
and a proc macro of its own.
