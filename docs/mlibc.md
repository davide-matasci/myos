# mlibc, the second C library

myos builds two C libraries from the same tree:

- **newlib** with libgloss/myos (`toolchain/newlib`) is the default. Every
  port, the smokes and the Rust `std` port are built against it.
- **[mlibc](https://github.com/managarm/mlibc)** (`toolchain/mlibc`,
  v7.0.0) is the second one. A port opts into it with `PORT_LIBC=mlibc` in
  its descriptor (`docs/ports.md`).

A program is a static PIE with the libc's crt0, unless it opts into
mlibc's shared objects ("Shared objects" below). The C
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
| `toolchain/mlibc/build.sh` | meson + ninja, clang 19 or later (C++23), for the three arches: `target/mlibc-<arch>/{include,lib}`, and the dynamic linker and the shared libc in `shared/` |
| `toolchain/mlibc/sysdeps/myos/` | the sysdeps (below) and the ABI headers (`toolchain/mlibc/README.md` says how they were made) |
| `toolchain/mlibc/sysdeps/myos/generic/rtld.cpp` | the dynamic linker's own sysdeps, on the raw syscalls |
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
- Programs are static by default, and need nothing of the sysroot on the
  running system.
- Without a feature macro a program sees POSIX 2008 with the XSI
  extensions, as with newlib (`patch.sh` widens mlibc's default).
  `_GNU_SOURCE` adds the GNU calls.

To put a port on mlibc, set `PORT_LIBC=mlibc` and add `mlibc` to
`PORT_DEPS`. A build script that uses `myos_write_cross_cc` sources its
`port.env` first, and gets a cc that builds against mlibc.

In CI mlibc is an ordinary port (kind `port`, not `toolchain`): the ports
job builds and caches it, and a port built on it builds it first when the
registry lacks it.

## Shared objects

A program may link the shared mlibc instead, and load objects at run time:

```sh
clang $(myos_mlibc_cflags x86_64) -O2 -c plugin.c -o plugin.o
myos_mlibc_link_shared x86_64 plugin.so plugin.o       # a shared object
myos_mlibc_link_dynamic x86_64 prog prog.o             # needs libc.so; --dynamic-linker=/lib/ld.so
```

- Every image carries the dynamic linker and the shared libc, `/lib/ld.so`
  and `/lib/libc.so` (mlibc's `rtld`), and the empty `libm.so`,
  `libpthread.so`, `libdl.so`, `librt.so`, `libutil.so` and `libresolv.so`
  a link's `-lm` names. They are built with the archives and kept apart
  from them (`target/mlibc-<arch>/shared`), so that a plain `-lc` goes on
  finding `libc.a`: static stays the default, and a program opts in by how
  it is linked.
- `exec` of a program with a `PT_INTERP` maps the interpreter beside it
  and starts there, as for a Linux program (`docs/linux-compat.md`
  "Dynamic linking"); the auxiliary vector gives the linker `AT_BASE`,
  `AT_PHDR`, `AT_ENTRY` and `AT_EXECFN` (the program's path, for
  `$ORIGIN`). The security policy's `exec` covers the interpreter.
- The linker looks for an object in the `DT_RUNPATH` of the object that
  names it (`$ORIGIN` is that object's directory), in `LD_LIBRARY_PATH`,
  then in `/lib` and `/usr/lib`. It maps each object's segments from its
  file, inside the process's mmap window: pages come in as
  they are touched, and an unwritten one is the page cache's.
- `dlopen`, `dlsym`, `dladdr`, `dl_iterate_phdr` and thread-local
  variables in loaded objects work on the three arches. `dlclose` succeeds
  and unloads nothing: an object stays until the program ends, when its
  destructors run (mlibc's).
- An object's undefined symbols resolve against the program too, which
  `myos_mlibc_link_dynamic` links with `--export-dynamic`.
- The C++ runtimes, GLib and the libraries over them are still static
  only: a shared object in C++ has no shared libc++ to link (issue #658
  lists what moving them takes).

`user/c-mlibc/dl_smoke.c` (the `dl` test) covers it: an object named in
`DT_NEEDED` and found through `$ORIGIN`, one loaded with `dlopen`, calls
in both directions, and a thread-local variable of each in two threads.

## The sysdeps

mlibc calls the system through its *sysdeps*, one C++ functor per call; a
call whose sysdep is missing fails with `ENOSYS`. The myos ones are a few
lines each over the shared C, under `toolchain/mlibc/sysdeps/myos/`
(`include/mlibc/sysdeps.hpp` lists them):

| File | What |
|------|------|
| `generic/sysdeps.cpp` | files, processes, memory, sockets, terminals, time, the event fds |
| `generic/entry.cpp`, `<arch>/crt0.S` | the program's start |
| `generic/thread.cpp`, `<arch>/thread-entry.S` | threads on the kernel's, and where a deferred cancellation may act: the cancellation points (`docs/threads.md`) |
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
Linux. The smoke (`user/c-mlibc/cxx_smoke.cpp`, the `cxx` test, one of
the mlibc smokes: libc++ is a library port and ships no program,
`docs/ports.md` "Cache keys") covers the runtime's main features.

## Ports on mlibc

Most ports are newlib's. On mlibc are the smokes, python
(`packages/python`, the first port on mlibc; its README says what of
CPython's build the libc shaped) and the libraries it and the GTK stack
need: LibreSSL, ncurses, SQLite, libedit, GLib, the X libraries, zlib,
the image and text libraries, GTK 3, libsoup. Some are built for both
libcs (`target/<name>-mlibc-<arch>`). The libraries are library ports,
keyed on what they compile against; their programs (`gio` and the
smokes) are packages of their own, `glib-tools`, `image-libs-smoke`,
`cairo-pango-smoke`, `gtk3-smoke` and `libsoup-smoke`, relinked when the
libc changes (`docs/ports.md` "Cache keys"). tcc's sysroot is newlib's.

## Limits

- Shared objects are mlibc's alone, and C only so far ("Shared objects");
  newlib has no dynamic linker.
- The Rust ports use no C library. A Rust program with C in it is built
  for the mlibc flavour of the target, `<arch>-unknown-myos-mlibc`
  (`docs/userspace.md` "`std` on mlibc"): static, and without unwinding
  so far.
- A fork while another thread holds one of mlibc's own locks (the
  allocator's, stdio's) hangs the child in its first `malloc` or
  `fprintf`, because mlibc's fork takes none of them (issue #544).
