# mlibc, the second C library

myos builds two C libraries from the same tree:

- **newlib** with libgloss/myos (`toolchain/newlib`) is the default. Every
  port, the smokes and the Rust `std` port are built against it.
- **[mlibc](https://github.com/managarm/mlibc)** (`toolchain/mlibc`,
  v7.0.0) is the second one. A port opts into it with `PORT_LIBC=mlibc` in
  its descriptor (`docs/ports.md`).

Both are static: a program is a static PIE with the libc's crt0. The C
smokes are built against both (`user/c`, `user/c-mlibc`) and the boot
tests run them both ways, so the kernel's interface is tested through two
libcs. Read this page when you port a program to mlibc or change the C
both libcs share.

## Where

| Where | What |
|-------|------|
| `toolchain/libmyos/` | the C both libcs share: the kernel's interface as libc functions (`__myos_read`, the signal trampolines, the tty code, ...), compiled against each libc's headers. The macros of `libc.h` decide under which libc a function gets its public name |
| `toolchain/newlib/libgloss/myos/` | what is newlib's alone: the reent hooks over `__myos_*`, signals, pthreads, stubs |
| `toolchain/mlibc/versions.env`, `fetch.sh` | the pins of mlibc, its meson subprojects and the compiler-rt builtins |
| `toolchain/mlibc/patch.sh` | installs the sysdeps and `libmyos` into mlibc's tree and edits upstream where myos needs other behaviour; its comments give each change's reason |
| `toolchain/mlibc/build.sh` | meson + ninja, clang 19 or later (C++23), for the three arches: `target/mlibc-<arch>/{include,lib}` |
| `toolchain/mlibc/sysdeps/myos/` | the sysdeps (below) and the ABI headers (`toolchain/mlibc/README.md` says how they were made) |
| `user/c-mlibc/` | the smokes of `user/c` built against mlibc, `/bin/etc/mlibc/*`, and their tests |
| `scripts/myos-c-userspace-lib.sh` | `myos_mlibc_cflags`, `myos_mlibc_link`, `PORT_LIBC` in `myos_write_cross_cc` |

## Building against it

```sh
./toolchain/mlibc/build.sh            # once: target/mlibc-{x86_64,aarch64,riscv64}
clang $(myos_mlibc_cflags x86_64) -O2 -c prog.c -o prog.o     # scripts/myos-c-userspace-lib.sh
myos_mlibc_link x86_64 prog prog.o    # crt0.o prog.o -lc -lbuiltins, a static PIE
```

- Everything is in `libc.a`; libm, libpthread and the rest are empty
  archives. `libbuiltins.a` holds the compiler-rt builtins a link needs,
  among them the software binary128 `long double` of aarch64 and riscv64.
- Programs are static, so the image carries none of the sysroot.
- Without a feature macro a program sees POSIX 2008 with the XSI
  extensions, as with newlib (`patch.sh` widens mlibc's default).
  `_GNU_SOURCE` adds the GNU calls.

To put a port on mlibc, set `PORT_LIBC=mlibc` and add `mlibc` to
`PORT_DEPS`. A build script that uses `myos_write_cross_cc` sources its
`port.env` first, and gets a cc that builds against mlibc.

In CI mlibc is an ordinary port (kind `port`, not `toolchain`): the ports
job builds and caches it, and a port built on it builds it first when the
registry lacks it.

## The sysdeps

mlibc calls the system through its *sysdeps*, one C++ functor per call; a
call whose sysdep is missing fails with `ENOSYS`. The myos ones are a few
lines each over the shared C, under `toolchain/mlibc/sysdeps/myos/`
(`include/mlibc/sysdeps.hpp` lists them):

| File | What |
|------|------|
| `generic/sysdeps.cpp` | files, processes, memory, sockets, terminals, time, the event fds |
| `generic/entry.cpp`, `<arch>/crt0.S` | the program's start |
| `generic/thread.cpp`, `<arch>/thread-entry.S` | threads on the kernel's (`docs/threads.md`) |
| `generic/signal.cpp` | the kernel's signal masks and `sigaction` records (`docs/signals.md`) behind mlibc's types, and the handler dispatch |
| `generic/dirent.cpp` | `readdir` over the kernel's whole-directory listing (`listdirat`) |

## What myos answers

myos does not use mlibc's Linux option, which wants a Linux kernel's
headers. What myos answers of it (`getrandom`, `getifaddrs`, `mount`,
`memfd_create`, ...) lives in the shared C.

- Signal numbers are the kernel's (newlib's BSD layout) in both libcs.
  `errno` values and `struct stat` layouts are each libc's own.
- `times` and `setpriority` are `ENOSYS`, as with libgloss: the kernel
  does not split a task's user and kernel time (issue #291) and has no
  priorities. `getrlimit` reads `/proc/self/limits`; `setrlimit` is
  `ENOSYS`.
- `sigtimedwait` with a timeout is `ENOSYS`: the kernel has no timed wait.
- mlibc's resolver reads `/etc/hosts`, then asks the nameserver of
  `/etc/resolv.conf`, or without that file the first `dns=` of `/net/ndb`
  (`docs/net.md`).

## Testing

`user/c-mlibc/test.sh` runs the tests of `user/c/test.sh` on the mlibc
builds, as `mlibc_<name>`. Each carries the `mlibc` tag and the newlib
test's name, so selecting or skipping a test takes both
(`docs/testing.md`).

```sh
cargo run -- test --tags mlibc
```

A test that passes with newlib and fails with mlibc points at the sysdeps
or the shared C. In CI a change under `toolchain/mlibc` or `user/c-mlibc`
selects the `mlibc` tests; one under `toolchain/libmyos` runs everything.

## C++

`toolchain/libcxx` builds LLVM's C++ runtimes against mlibc: libc++,
libc++abi (merged into `libc++.a`) and libunwind, static, in
`target/libcxx-<arch>`. Its LLVM release is the one mlibc's compiler-rt
comes from (`LLVM_TAG`).

```sh
./toolchain/libcxx/build.sh                                       # once: target/libcxx-<arch>
clang++ $(myos_libcxx_cxxflags x86_64) -std=c++23 -O2 -c prog.cpp -o prog.o
myos_libcxx_link x86_64 prog prog.o    # mlibc's link with libc++ and libunwind
```

C++ programs get exceptions and RTTI, threads and `thread_local`, the
locales, `<filesystem>`, `<random>` and `<chrono>`, but no time zone
database (there is no tzdata in the image). A C++ compile is hosted, not
freestanding as the C one is, and defines `_GNU_SOURCE` as g++ does on
Linux. The smoke (`toolchain/libcxx/smoke.cpp`, the `cxx` test) covers
the runtime's main features.

## Ports on mlibc

Most ports are newlib's. On mlibc are the smokes, python
(`packages/python`, the first port on mlibc; its README says what of
CPython's build the libc shaped) and the libraries it and the GTK stack
need: LibreSSL, ncurses, SQLite, libedit, GLib, the X libraries, zlib,
the image and text libraries, GTK 3, libsoup. Some are built for both
libcs (`target/<name>-mlibc-<arch>`). tcc's sysroot is newlib's.

## Limits

- No shared objects or `dlopen`: mlibc is built static, as newlib is.
- No Rust `std` on mlibc: the Rust port is newlib's.
- A fork while another thread holds one of mlibc's own locks (the
  allocator's, stdio's) hangs the child in its first `malloc` or
  `fprintf`, because mlibc's fork takes none of them (issue #544).
