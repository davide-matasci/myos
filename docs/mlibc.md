# mlibc, the second C library

myos builds two C libraries from the same tree: **newlib** with
libgloss/myos (`toolchain/newlib`, the default: every port, the smokes
and the Rust `std` port are built against it) and
**[mlibc](https://github.com/managarm/mlibc)** (`toolchain/mlibc`, v7.0.0),
which a port opts into with `PORT_LIBC=mlibc` in its descriptor
(`docs/ports.md`). Both are static: a program is a static PIE with the
libc's crt0, as every myos program is. The C smokes are built against
both (`user/c`, `user/c-mlibc`), and the boot tests run them both ways:
the kernel's interface seen through two libcs.

## Where

| Where | What |
|-------|------|
| `toolchain/libmyos/` | the C both libcs share: the kernel's interface as libc functions (`__myos_read`, `__myos_openat`, `__myos_socket`, the signal trampolines, the tty code, ...), written to the libc's own headers, answering as libgloss always did (`-1` with `errno`). `libc.h` gives a function its public name: `MYOS_PUBLIC(name)` in both libcs (what neither libc has of its own: `getifaddrs`, `mount`, `memfd_create`, ...), `MYOS_ALIAS(name)` / `MYOS_WEAK_ALIAS(name)` in newlib only (mlibc has the function and calls the sysdep) |
| `toolchain/newlib/libgloss/myos/` | what is newlib's alone: the reent hooks (`hooks.c`: `_read`, `_open`, ... over `__myos_*`), `signal.c` (the `sigaction` family over `sigtramp.c`'s kernel calls), `pthread.c`, the stubs, newlib's missing POSIX pieces |
| `toolchain/mlibc/versions.env`, `fetch.sh` | the pins: mlibc's tag and the revisions of the meson subprojects its build takes (frigg, libsmarter, the freestanding C and C++ headers), fetched into `target/mlibc-src`; the compiler-rt builtins (`target/mlibc-compiler-rt`) |
| `toolchain/mlibc/patch.sh` | installs the sysdeps and `libmyos` into the tree, links the Linux ABI bits where myos has no own, and edits what upstream needs told (python replacements, as newlib's `patch.sh`): the sysdeps in `meson.build` (and no ifuncs: the kernel relocates a static PIE itself and applies `RELATIVE` relocations only), the default feature macros, `memfd_create`'s declaration, riscv64's soft-float `setjmp` |
| `toolchain/mlibc/build.sh` | meson + ninja, clang 19 or later (C++23), for the three arches: `target/mlibc-<arch>/{include,lib}` |
| `toolchain/mlibc/sysdeps/myos/` | the sysdeps (below) and the ABI headers (`include/abi-bits`: the signal numbers and flags, termios, the ioctl requests, the memory flags; `toolchain/mlibc/README.md` says how they were made) |
| `user/c-mlibc/` | the `c-smokes-mlibc` port: the smokes of `user/c` built against mlibc, `/bin/etc/mlibc/*`, and their tests |
| `scripts/myos-c-userspace-lib.sh` | `myos_mlibc_cflags`, `myos_mlibc_link`, `PORT_LIBC` in `myos_write_cross_cc`, the hashes |

## Building against it

```sh
./toolchain/mlibc/build.sh            # once: target/mlibc-{x86_64,aarch64,riscv64}
clang $(myos_mlibc_cflags x86_64) -O2 -c prog.c -o prog.o     # scripts/myos-c-userspace-lib.sh
myos_mlibc_link x86_64 prog prog.o    # crt0.o prog.o -lc -lbuiltins, a static PIE
```

The sysroot: `include/` (mlibc's headers, `myos/` with the raw syscall
headers both libcs carry, `sys/myos_extra.h`), `lib/crt0.o` (`crt1.o`
is the same object, for a driver that asks for the hosted name),
`crti.o` and `crtn.o` (libgloss's), `libc.a` (libm, libpthread, librt,
libdl, ... are empty archives: everything is in libc.a) and
`libbuiltins.a`, the compiler-rt builtins a link needs beside libc.a: the
IEEE binary128 `long double` arithmetic of aarch64 and riscv64 (mlibc's
printf and strtod use it), riscv64's soft-float double and float
arithmetic (userspace is rv64imac), the complex multiplications. The
image carries the sysroot at `/lib/mlibc` (newlib's is `/lib/newlib`).

A program is compiled with mlibc's headers before clang's (freestanding,
clang's `limits.h` does not include the libc's after itself) and without a
feature macro sees POSIX 2008 with the XSI extensions, as a program
compiled against newlib does (mlibc's own default shows less:
`patch.sh`); `_GNU_SOURCE` adds the GNU calls (`memfd_create`, ...).

A port: `PORT_LIBC=mlibc` and `mlibc` in `PORT_DEPS`; a build script
that uses `myos_write_cross_cc` sources its `port.env` first, and the cc
it gets compiles and links against mlibc. CI caches mlibc like a port
(`scripts/ci-registry.sh`, package `mlibc`): the job of a port built on
it builds it when the registry lacks it, as any dependency.

## The sysdeps

mlibc calls the system through its *sysdeps*, one C++ functor per call
(`Sysdeps<Openat>`, ...); a call whose sysdep a system does not implement
fails with `ENOSYS`. The myos ones (`sysdeps/myos/include/mlibc/sysdeps.hpp`
lists them) are a few lines each over the shared C:

| File | What |
|------|------|
| `generic/sysdeps.cpp` | files, directories, processes, memory, sockets, terminals, time, the event fds (eventfd, timerfd, epoll, signalfd), the hostname, `reboot` |
| `generic/entry.cpp`, `<arch>/crt0.S` | the program's start: `_start` hands `main` and the entry stack to mlibc's startup, which parses it (argc, argv, envp; the auxiliary vector is empty: mlibc's static startup finds its own program headers through `__ehdr_start`), sets up the TLS and runs the constructors |
| `generic/thread.cpp`, `<arch>/thread-entry.S` | threads on the kernel's (`docs/threads.md`): `thread_spawn`, the futex (`wait_addr`, `wake_addr`), the thread pointer (`set_tp` on x86_64, `tpidr_el0`, `tp`: mlibc's TCB lies at the pointer on x86_64, 16 bytes before its end on aarch64, at its end on riscv64) |
| `generic/signal.cpp` | the kernel's 32-bit masks and `sigaction` records (`docs/signals.md`) behind mlibc's `sigset_t`; the handler dispatch the trampoline calls, with the siginfo the kernel's frame holds; `sigtimedwait` without a timeout (`ENOSYS` with one: the kernel has no timed wait) |
| `generic/dirent.cpp` | the kernel lists a directory's names at once (`listdirat`); mlibc reads `dirent64` records a buffer at a time: the listing is read at the first read on a directory fd, handed out record by record (`d_type` and `d_ino` from a stat, `.` and `..` left out, as newlib's `readdir`), and dropped at its end, a rewind or the fd's close |

Not mlibc's Linux option (it wants a Linux kernel's headers): what myos
answers of it (`getrandom`, `getifaddrs`, `mount`, the mount table,
`memfd_create`, the device number macros) is in the shared C, with the
headers in `sysdeps/myos/include`. `times` and `setpriority` are `ENOSYS`
(no CPU accounting, no priorities: libgloss says the same), `getpriority`
is 0. Signal numbers are the kernel's (newlib's BSD layout) in both libcs;
`errno` values and `struct stat` layouts are each libc's own, the shared
C being compiled against each libc's headers.

## Testing

`user/c-mlibc/test.sh` runs every test of `user/c/test.sh` on the mlibc
builds of the same smokes, as `mlibc_<name>` with the `mlibc` tag
(`docs/testing.md`; the tests that run no smoke program, and `ctor_tcc`,
whose program tcc links against newlib, are not repeated):

```sh
cargo run -- test --tags mlibc
```

A test that passes with newlib and fails with mlibc points at the sysdeps
or the shared C. In CI a change under `toolchain/mlibc` or `user/c-mlibc`
selects the `mlibc` tests; one under `toolchain/libmyos` runs everything
(both libcs are built on it).

## Not there

No shared objects or `dlopen` (mlibc is built static, as newlib is); no
Rust `std` on mlibc (the Rust port is newlib's); no port other than the
smokes is built against it yet; tcc's own sysroot is newlib's.
