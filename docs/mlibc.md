# mlibc, the second C library

myos builds two C libraries from the same tree:

- **newlib** with libgloss/myos (`toolchain/newlib`) is the default. Every
  port, the smokes and the Rust `std` port are built against it.
- **[mlibc](https://github.com/managarm/mlibc)** (`toolchain/mlibc`,
  v7.0.0) is the second one. A port opts into it with `PORT_LIBC=mlibc` in
  its descriptor (`docs/ports.md`).

Both are static: a program is a static PIE with the libc's crt0, as every
myos program is. The C smokes are built against both (`user/c`,
`user/c-mlibc`), and the boot tests run them both ways. So the kernel's
interface is tested through two libcs. Read this page when you port a
program to mlibc or change the C both libcs share.

## Where

| Where | What |
|-------|------|
| `toolchain/libmyos/` | the C both libcs share: the kernel's interface as libc functions (`__myos_read`, `__myos_openat`, `__myos_socket`, the signal trampolines, the tty code, ...). It is written to the libc's own headers and answers as libgloss always did (`-1` with `errno`). `libc.h` gives a function its public name (below) |
| `toolchain/newlib/libgloss/myos/` | what is newlib's alone: the reent hooks (`hooks.c`: `_read`, `_open`, ... over `__myos_*`), `signal.c` (the `sigaction` family over `sigtramp.c`'s kernel calls), `pthread.c`, the stubs, newlib's missing POSIX pieces |
| `toolchain/mlibc/versions.env`, `fetch.sh` | the pins: mlibc's tag and the revisions of the meson subprojects its build takes (frigg, libsmarter, the freestanding C and C++ headers), fetched into `target/mlibc-src`; the compiler-rt builtins (`target/mlibc-compiler-rt`) |
| `toolchain/mlibc/patch.sh` | installs the sysdeps and `libmyos` into the tree, links the Linux ABI bits where myos has none of its own, and edits what upstream needs told (below) |
| `toolchain/mlibc/build.sh` | meson + ninja, clang 19 or later (C++23), for the three arches: `target/mlibc-<arch>/{include,lib}` |
| `toolchain/mlibc/sysdeps/myos/` | the sysdeps (below) and the ABI headers (`include/abi-bits`: the signal numbers and flags, termios, the ioctl requests, the memory flags; `toolchain/mlibc/README.md` says how they were made) |
| `user/c-mlibc/` | the `c-smokes-mlibc` port: the smokes of `user/c` built against mlibc, `/bin/etc/mlibc/*`, and their tests |
| `scripts/myos-c-userspace-lib.sh` | `myos_mlibc_cflags`, `myos_mlibc_link`, `PORT_LIBC` in `myos_write_cross_cc`, the hashes |

The macros of `libc.h` that name a shared function:

| Macro | Public name in | When |
|-------|----------------|------|
| `MYOS_PUBLIC(name)` | both libcs | what neither libc has of its own: `getifaddrs`, `mount`, `memfd_create`, `madvise`, ... |
| `MYOS_ALIAS(name)` / `MYOS_WEAK_ALIAS(name)` | newlib only | mlibc has the function and calls the sysdep |
| `MYOS_MLIBC_PUBLIC(name)` | mlibc only | newlib has its own: the `arc4random` family over `getrandom` |

`patch.sh` edits upstream with python replacements, as newlib's
`patch.sh` does. It changes:

- the sysdeps in `meson.build`, with no ifuncs: the kernel relocates a
  static PIE itself and applies `RELATIVE` relocations only;
- the default feature macros;
- `memfd_create`'s and `madvise`'s declarations;
- a thread's misuse of its locks, to answer as libgloss does and as the
  pthread smoke checks: `EBUSY`, `EPERM`, `EDEADLK` from a mutex, a
  read-write lock or a join, instead of a panic or a wait. A cancelled
  condition wait's cleanup handlers run with the mutex locked again;
- a failed stdio write, to set the stream's error indicator instead of
  logging a line per failure;
- `hstrerror`'s messages;
- `inet_pton`'s check of an IPv6 address's embedded IPv4 octets: it
  checks each octet for a leading zero on its own. Upstream checks the
  octets against the hex group before them, and so refuses
  `::ffff:10.0.2.2`.

## Building against it

```sh
./toolchain/mlibc/build.sh            # once: target/mlibc-{x86_64,aarch64,riscv64}
clang $(myos_mlibc_cflags x86_64) -O2 -c prog.c -o prog.o     # scripts/myos-c-userspace-lib.sh
myos_mlibc_link x86_64 prog prog.o    # crt0.o prog.o -lc -lbuiltins, a static PIE
```

The sysroot holds:

- `include/`: mlibc's headers, `myos/` with the raw syscall headers both
  libcs carry, and `sys/myos_extra.h`;
- `lib/crt0.o`, with `crt1.o` as the same object for a driver that asks
  for the hosted name;
- `crti.o` and `crtn.o` (libgloss's);
- `libc.a`. libm, libpthread, librt, libdl, ... are empty archives:
  everything is in libc.a;
- `libbuiltins.a`, the compiler-rt builtins a link needs beside libc.a:
  the IEEE binary128 `long double` arithmetic of aarch64 and riscv64
  (mlibc's printf and strtod use it) and the complex multiplications.

Its programs are static, so the image carries none of the sysroot.
newlib's sysroot is in the tcc package, at `/lib/newlib`.

How a program is compiled:

- with mlibc's headers before clang's. The compile is freestanding, and
  clang's `limits.h` does not include the libc's after itself;
- without a feature macro, it sees POSIX 2008 with the XSI extensions, as
  a program compiled against newlib does. mlibc's own default shows less
  (`patch.sh` changes it). `_GNU_SOURCE` adds the GNU calls
  (`memfd_create`, ...);
- `myos_mlibc_cflags` includes mlibc's `stdc-predef.h` first, as clang
  does glibc's only for a Linux target. Its `__STDC_ISO_10646__` says a
  `wchar_t` holds Unicode (libedit wants it).

To put a port on mlibc, set `PORT_LIBC=mlibc` and add `mlibc` to
`PORT_DEPS`. A build script that uses `myos_write_cross_cc` sources its
`port.env` first, and the cc it gets compiles and links against mlibc.

In CI mlibc is a port like any other: kind `port`, not `toolchain`, so it
has no job of its own in the workflow. The ports job builds and caches it
(`scripts/ci-registry.sh`, package `mlibc`). The job of a port built on it
builds it itself when the registry lacks it, as any dependency.

## The sysdeps

mlibc calls the system through its *sysdeps*, one C++ functor per call
(`Sysdeps<Openat>`, ...). A call whose sysdep a system does not implement
fails with `ENOSYS`. The myos ones are a few lines each over the shared C;
`sysdeps/myos/include/mlibc/sysdeps.hpp` lists them.

| File | What |
|------|------|
| `generic/sysdeps.cpp` | files, directories, processes, memory, sockets, terminals, time, the event fds (eventfd, timerfd, epoll, signalfd), the hostname, `reboot` |
| `generic/entry.cpp`, `<arch>/crt0.S` | the program's start: `_start` hands `main` and the entry stack to mlibc's startup, which parses it (argc, argv, envp; mlibc's static startup finds its own program headers through `__ehdr_start`, and `getauxval` reads the kernel's auxiliary vector: the program headers, the page size, the ids and `AT_SECURE`, 0), sets up the TLS and runs the constructors |
| `generic/thread.cpp`, `<arch>/thread-entry.S` | threads on the kernel's (`docs/threads.md`): `thread_spawn`, the futex (`wait_addr`, `wake_addr`), the thread pointer (`set_tp` on x86_64, `tpidr_el0`, `tp`: mlibc's TCB lies at the pointer on x86_64, 16 bytes before its end on aarch64, at its end on riscv64) |
| `generic/signal.cpp` | the kernel's 32-bit masks and `sigaction` records (`docs/signals.md`) behind mlibc's `sigset_t`; the handler dispatch the trampoline calls, with the siginfo the kernel's frame holds; `sigtimedwait` without a timeout (`ENOSYS` with one: the kernel has no timed wait) |
| `generic/dirent.cpp` | the kernel lists a directory's names at once (`listdirat`); mlibc reads `dirent64` records a buffer at a time: the listing is read at the first read on a directory fd, handed out record by record (`d_type` and `d_ino` from a stat, `.` and `..` left out, as newlib's `readdir`), and dropped at its end, a rewind or the fd's close |

## What myos answers

myos does not use mlibc's Linux option, which wants a Linux kernel's
headers. What myos answers of it lives in the shared C, with the headers
in `sysdeps/myos/include`: `getrandom`, `getifaddrs`, `mount`, the mount
table, `memfd_create`, the device number macros.

- `times` and `setpriority` are `ENOSYS` (libgloss says the same). The
  kernel counts each task's CPU time, but not its user time apart from its
  kernel time (issue #291), and has no priorities. `getpriority` is 0.
- `getrlimit` reads `/proc/self/limits`. Only the main thread's stack and
  the fd table have a limit; `setrlimit` is `ENOSYS`.
- `pthread_attr_get_np` of the main thread (WebKit's question) reads its
  stack from `/proc/self/stack` (`docs/proc.md`). A thread mlibc started
  knows its own.
- Signal numbers are the kernel's (newlib's BSD layout) in both libcs.
  `errno` values and `struct stat` layouts are each libc's own, because
  the shared C is compiled against each libc's headers.
- mlibc's resolver is its own. It reads `/etc/hosts` for `localhost`
  (libgloss knows the name itself), then asks DNS: the nameserver of
  `/etc/resolv.conf`, or without that file the first `dns=` of
  `/net/ndb`, as libgloss does (`patch.sh`). `/etc/services` names the
  DNS port it looks up.
- A signal fd's records are 16 bytes from the kernel (newlib's
  `struct signalfd_siginfo`). The shared read (`evfd.c`) widens them to
  mlibc's Linux-sized struct.

## Testing

`user/c-mlibc/test.sh` runs every test of `user/c/test.sh` on the mlibc
builds of the same smokes, as `mlibc_<name>`. Each carries the `mlibc` tag
and the newlib test's name as a tag, so that a selection or a skip by that
name takes both (`docs/testing.md`). The tests that run no smoke program
are not repeated.

```sh
cargo run -- test --tags mlibc
```

A test that passes with newlib and fails with mlibc points at the sysdeps
or the shared C. In CI a change under `toolchain/mlibc` or `user/c-mlibc`
selects the `mlibc` tests. A change under `toolchain/libmyos` runs
everything, because both libcs are built on it.

## C++

`toolchain/libcxx` builds LLVM's C++ runtimes against mlibc, at the LLVM
release mlibc's compiler-rt builtins come from (`LLVM_TAG`). It builds
libc++, libc++abi (merged into `libc++.a`) and libunwind, static, in
`target/libcxx-<arch>` (`include/c++/v1`, `lib/`).

```sh
./toolchain/libcxx/build.sh                                       # once: target/libcxx-<arch>
clang++ $(myos_libcxx_cxxflags x86_64) -std=c++23 -O2 -c prog.cpp -o prog.o
myos_libcxx_link x86_64 prog prog.o    # mlibc's link with libc++ and libunwind
```

C++ programs get:

- exceptions and RTTI. libunwind's DWARF unwinder finds a program's
  frames through mlibc's `dl_iterate_phdr`;
- threads and `thread_local` on mlibc's pthreads;
- the locales, through mlibc's `newlocale`/`uselocale` and `*_l`
  functions;
- `<filesystem>`, `<random>` (`random_device` reads `/dev/urandom`) and
  `<chrono>`, but no time zone database (there is no tzdata in the image).

How the build fits libc++ to myos:

- libc++ knows no platform for myos's bare triple. The build takes
  mlibc's POSIX locale functions as they are, libc++'s own ctype table
  (`_LIBCPP_PROVIDES_DEFAULT_RUNE_TABLE`, in `__config_site`) and pthreads.
- A C++ compile is hosted, not freestanding as `myos_mlibc_cflags` is for
  C. A freestanding compile mangles `main` like any function and makes
  libc++ a freestanding library.
- A C++ compile defines `_GNU_SOURCE`, as g++ does on Linux. libc++'s
  headers call the `*_l` functions mlibc declares under it.

The smoke (`toolchain/libcxx/smoke.cpp`, `/bin/etc/mlibc/cxx_smoke`, the
`cxx` test) throws through frames and across threads. It uses RTTI, a
global's constructor, threads, `thread_local`s, atomic waits, the
containers, iostream, `std::format`, `<filesystem>`, `<chrono>` and
`<random>`.

## Ports on mlibc

The ports are newlib's, except these:

- the smokes;
- python (`packages/python`), the first port on mlibc. Its README says
  what of CPython's build the libc shaped;
- the libraries python links: LibreSSL and ncurses (built for both libcs,
  with `target/<name>-mlibc-<arch>` the mlibc build), SQLite and libedit;
- GLib (`packages/glib`), with PCRE2's mlibc build and the libraries
  mlibc lacks: GNU libiconv for `iconv`, proxy-libintl for `gettext`;
- the X client libraries and the font stack (`packages/x11-libs` and
  `x11-xft`, built for both libcs);
- zlib (`ports/zlib`, both libcs);
- the image libraries (`packages/image-libs`);
- the text and drawing libraries (`packages/cairo-pango`, HarfBuzz in C++
  over libc++);
- GTK 3 (`packages/gtk3`) and the network libraries (`packages/libsoup`).

tcc's own sysroot is newlib's.

## Limits

- No shared objects or `dlopen`: mlibc is built static, as newlib is.
- No Rust `std` on mlibc: the Rust port is newlib's.
- A fork while another thread holds one of mlibc's own locks (the
  allocator's, stdio's) hangs the child in its first `malloc` or
  `fprintf`. mlibc's fork takes none of them, where libgloss's takes
  newlib's (issue #544; the pthread smoke skips that check on mlibc).
