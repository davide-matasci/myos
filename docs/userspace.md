# Userspace

This page covers the native userspace: the syscall interface, the boot from
init to a login shell, and the Rust and C runtimes programs link against
(newlib with libgloss, or mlibc). Read it before you port a program or add
a syscall. The syscalls are in `kernel/src/user/`.

## Syscalls

A syscall takes up to six arguments in registers:

| Arch | Registers |
|------|-----------|
| x86-64 | rdi, rsi, rdx, r10, r8, r9 |
| aarch64 | x0-x5 |
| riscv64 | a0-a5 |

The numbers only grow. A removed call leaves its number unused. libc wraps
every call; the list below names the native calls where they differ from
POSIX.

The kernel's entry trusts no register a program can load. On x86-64 the
per-CPU state `syscall_entry` needs is behind `IA32_KERNEL_GS_BASE`,
brought into GS with `swapgs` for the instructions that address it
(interrupts off) and out again: `%gs` and its base are the program's,
null selector included, and nothing else in the kernel uses GS (the CPU's
number comes from `IA32_TSC_AUX`). An address a program chooses that the
kernel loads in ring 0 (a thread pointer into the FS base, a thread's or
an image's entry for `iretq`) is loaded as one that faults in the program
when it is not canonical (`canonical_or_fault`, `arch/x86/user.rs`);
`SYSCALL`'s flag mask clears TF, DF, NT and IF, and a signal frame's
rflags keep only the bits a program may set. aarch64 keeps its CPU number
in `tpidr_el1` and riscv64 reloads `tp` from the kernel stack on every
trap from U-mode. `fault_smoke` has a program try each on x86-64.

### Path calls

Every path call (`kernel/src/user/at.rs`) is `*at` style: it names its file
by a directory fd and a path relative to it. `AT_FDCWD` (-100) is the cwd,
an absolute path ignores the fd, and `AT_EMPTY_PATH` with an empty path is
the fd's own file. libc's plain calls (`open`, `stat`, `chdir`, ...) and
fd calls (`fstat`, `fchdir`, `fexecve`, ...) are these on `AT_FDCWD` or on
the fd itself.

The calls are `openat`, `statat`, `mknodat` (a directory or a FIFO),
`symlinkat`, `unlinkat`, `renameat`, `readlinkat`, `utimensat`, `chdirat`,
`listdirat` (the names, one per line), `execat` (an ELF or a `#!` script)
and `statfsat` (how full the filesystem is, for `statvfs` and `df`).
tmpfs and ext2 keep file times; other filesystems report 0 and refuse to
set them.

Each call holds the VFS tree lock from resolving its paths to acting on
them, so a directory fd stands for its directory whatever is renamed
meanwhile. The cwd follows a rename of its directory too. A directory fd
whose directory the caller's namespace cannot name is a capability: paths
resolve beneath it only (`docs/security.md`).

`openat` flags:

- `O_CREAT|O_EXCL` creates the file or fails with `EEXIST` in one step;
  a symlink at the name counts as taken.
- `O_CLOEXEC` closes the fd at exec.
- `O_NOFOLLOW` refuses a symlink as the last component (`ELOOP`).
- `O_DIRECTORY` refuses anything but a directory (`ENOTDIR`).
- A directory opened to write, create or truncate fails with `EISDIR`.

So a walk of a tree with `openat` cannot be led out of it by a symlink
swapped in.

### File I/O and locks

- `pread(fd, buf, len, offset, flags)` and `pwrite` are also read and
  write: without `FILE_AT` they use and advance the file position.
- `ftruncate` cuts a file or grows it with zeros.
- `fdflags` gets or sets close-on-exec (`fcntl`'s `F_GETFD`/`F_SETFD`).
  `dupfd` and `pipe` take the flag too, and `DUP_EXACT` makes `dupfd` a
  `dup2` or `dup3`.
- `flock(fd, op)` and `lockctl(fd, cmd, range)` back libc's `flock`,
  `fcntl` record locks and `lockf`, and Rust's `File::lock`. With
  `LOCKCTL_OFD` a record lock belongs to the open file description.
  `docs/fs.md` has the lock semantics.

### Other calls

- Memory: `brk`, `mmap`, `munmap`, `mprotect`, `msync`, `madvise`.
  `MADV_DONTNEED` frees the pages, which read as zero or as their file
  next time. Other advice is ignored.
- Threads (`docs/threads.md`): `thread_spawn`, `thread_exit`,
  `wait_addr`, `wake_addr`, `gettid`, `set_tp`, `yield`.
- Time: `settimeofday` sets the wall clock until the next boot (the RTC
  keeps its own). `clock_monotonic` gives the nanoseconds since boot.
- `power`: power off, reboot or halt (`docs/power.md`).
- Security (`docs/security.md`): `setuser` runs as a user, checked against
  the password. `ns` narrows the caller's namespace; libc's `chroot` is a
  namespace of one binding. `policy_load`.

There is no `ioctl`: a device's state is its `ctl` file (`docs/tty.md`).
`read` and `write` are `pread` and `pwrite` without `FILE_AT`, and the
path calls before `*at` are gone.

## Init and the shell

`user/init` is PID 1, `/bin/custom/init` in the initramfs. It:

1. smoke-tests `fork` and exec;
2. runs `mount -a` (`docs/fs.md`);
3. forks netd, the sound server sndd (`docs/sound.md`) and the boot script
   `/etc/rc`, which runs in the background and ends with
   `[ OK ] boot complete` (`docs/install.md`);
4. forks a getty on the serial port's `/dev/ttyS0` and on `/dev/tty1` to
   `ttyN`, the framebuffer's text screens. `N` comes from `/etc/ttys`
   (`/data/etc/ttys` overrides it). Alt+1 to Alt+9 switch screens, and
   Alt+0 shows the console's (`docs/tty.md`);
5. restarts each getty when it exits.

getty execs `/bin/ubase/login`. The kernel checks the password
(`setuser`; `root` has none), and the process becomes that user in its
login domain (`root`: `admin`). login then execs the shell,
`/bin/custom/sh`: oksh 7.9 (also `/bin/sh`), with this `PATH`:

```
/bin/sbase:/bin/coreutils:/bin/ubase:/bin/custom:/bin/tcc:/bin/std:/bin/etc
```

The shell keeps its history in the file `HISTFILE` names when it is set
(`export HISTFILE=$HOME/.sh_history`, say): each command is appended, and
the next shell reads them back (`fc -l`, the arrow keys). It is not set by
default. oksh's `KSH_VERSION` (`SH_VERSION` as `sh`) and `OKSH_VERSION` are
set. Ctrl-L redraws the line; it does not clear the screen (oksh is built
without curses).

Programs not in the image (vim, git, `btm` from bottom, ...) are packages:
`get-myos vim` installs one as an app, `run-myos vim` runs it
(`docs/packages.md`). `ps` lists the processes (`docs/proc.md`).

### Terminals and event files

- The framebuffer terminal handles scroll regions, 256/RGB colors and
  UTF-8 with box-drawing, block and braille glyphs. `TERMCAP=/lib/termcap`
  (`ports/termcap`) and termios raw mode serve full-screen TUIs. It moves
  the cursor by row and column (CUP, CHA, VPA, CNL/CPL), erases, inserts
  and deletes cells and lines (ECH, ICH, DCH, IL, DL), and skips what it has
  no use for: OSC, DCS, APC and PM strings (titles, hyperlinks, queries),
  character set designations (`ESC ( B`), and CSI queries and modes with a
  `>`, `<` or `=` marker or an intermediate byte. It answers no query.
- A terminal is a directory with `data` and `ctl` (its termios and window
  size as text): the console at `/dev/console/`, the ptys at
  `/dev/pts/N/` from `/dev/pts/clone` (`docs/tty.md`).
- eventfd, timerfd, epoll and signalfd are directories the same way:
  `/dev/event/N/`, `/dev/timer/N/`, `/dev/wait/N/` and `/dev/sig/N/`, each
  from its `clone` file, with libgloss's usual headers on top
  (`docs/events.md`).

## Rust userspace

- `user/lib` is the no-`std` library: syscalls, `brk` and a bump
  `GlobalAlloc`.
- `std` programs link the prebuilt sysroot
  (`toolchain/std/build-sysroot.sh`; myos builds it too, "Building myos
  on myos"). The port has `std::fs` (no hard links
  or permissions; `canonicalize` through the kernel's `realpath`),
  `std::thread` on native threads with futex locks on `wait_addr` /
  `wake_addr`, and dlmalloc on anonymous `mmap` regions as the heap.
- The kernel has one failure value, so std works out each call's
  `ErrorKind` the way libgloss does.
- Their tests are the `/bin/std/*` programs (`fs`, `thread`, `alloc`) and
  `user/heap`, in the tests module (`docs/testing.md`).

### `std` on mlibc

The sysroot holds `std` twice per arch, two different ports:

- `<arch>-unknown-myos`, the target of the Rust ports of the image, is
  the port above: `std` on the kernel's calls, with no C library at all.
- `<arch>-unknown-myos-mlibc` (`target_env = "mlibc"`) is a **unix
  target** (`target_family = "unix"`): `std`'s own unix code, the one
  Linux and the BSDs run, over mlibc through the `libc` crate. It is the
  flavour for a program with C or C++ in it, and for one that needs what
  a unix `std` has: a compiler, a build tool.

| | `<arch>-unknown-myos` | `<arch>-unknown-myos-mlibc` |
|---|---|---|
| start | std's own `_start` | mlibc's startup (`__mlibc_entry`), then `main`; its value is the exit status |
| heap | dlmalloc on `mmap` | `malloc` |
| threads | `thread_spawn`, std's stacks | `pthread_create`, named |
| thread-locals | a table at the thread pointer | `pthread` keys |
| environment | none | the C library's (`environ`, `setenv`) |
| arguments | at most 16 of 127 bytes | any |
| processes | `spawn` | `fork` and `exec`: pipes, a directory, an environment, `kill`, the signal a child ended by |
| files | the kernel's calls | the C library's; `std::os::unix::fs` (modes, inodes, `symlink`, positional reads, locks) |
| network | none | `std::net` (TCP, UDP, name lookup) and unix sockets |
| panic | aborts | unwinds, through LLVM's libunwind (`toolchain/libcxx`) |
| `cfg(unix)`, `std::os::unix` | no | yes |

So a program of the mlibc flavour has one heap with its C code (Rust
frees what C allocated and the other way round), the thread pointer is
the libc's on every thread (C's `errno` and `__thread` variables work on
a `std::thread`), and a thread C starts with `pthread_create` may run
Rust.

A panic there unwinds the stack: `catch_unwind` stops it, the values of
the frames it leaves are dropped, a thread that panics ends alone and
`join` gives its payload. The panic runtime is the unix targets' (the gcc
personality over `_Unwind_*`), found through the program's
`PT_GNU_EH_FRAME`. Only the unwinding runtime is in the sysroot for this
flavour: a profile with `panic = "abort"` does not build.

How it is made (`toolchain/std/patches/`, applied by `wire-myos.py` to
the library tree of the pinned nightly):

- `std-unix.patch` adds `target_os = "myos"` to `std`'s unix code where
  it lists systems (which calls exist, which constants), as a port of
  `std` to a new unix does. `wire-myos.py`'s own wiring, the other
  flavour's, leaves the mlibc targets out.
- `libc-myos.patch` adds myos to the `libc` crate (`std` calls the C
  library through it): the types, structs, constants and functions of
  mlibc on myos, a `linux_like` system (mlibc's ABI headers are Linux's)
  with myos's own signal numbers, `sa_flags` and a few layouts. It is
  applied to the copy `std`'s workspace vendors, which the workspace is
  then pointed at.
- Both are adapted from Managarm's port, another system on mlibc
  (`THIRD_PARTY_NOTICES.md`).
- The numbers in the crate are written by hand, and a wrong one is a
  wrong signal or field at run time. `toolchain/std/abi-check.sh`
  compares them all with mlibc's headers on the three arches (about 1300
  constants, sizes and field offsets each): run it after a change to the
  patch, to the ABI headers (`toolchain/mlibc/sysdeps/myos/include/abi-bits`)
  or a bump of mlibc or the nightly.
- The program's entry is a weak `_start` in `std`
  (`toolchain/std/pal/unix/myos.rs`), mlibc's crt0 in three instructions:
  a program needs no startup object in its link.

What the system does not have shows as on any unix without it: a hard
link is refused (`EPERM`, `docs/fs.md`; cargo and the like copy), a
mode is taken and not kept (`docs/security.md`), a path is UTF-8.

A program for it is built with the same sysroot, its link given mlibc's
directory and libunwind's (the target's spec adds `-lunwind -lc
-lbuiltins`):

```sh
RUSTFLAGS="-L native=$PWD/target/mlibc-x86_64/lib -L native=$PWD/target/libcxx-x86_64/lib" \
  cargo build --release --target targets/x86_64-unknown-myos-mlibc.json   # as user/std-mlibc/build.sh
```

A crate that names the `libc` crate itself gets crates.io's, which does
not know myos: it needs the patched one (`[patch.crates-io]`, with
`toolchain/std/patches/libc-myos.patch` applied to 0.2.189).

#### Rust shared objects

The flavour's programs are static by default, and it also builds Rust
`dylib`s (`dynamic-linking` in the target's spec), over mlibc's shared
objects (`docs/mlibc.md` "Shared objects"): a program may hold its std in
a shared object and load others at run time, as a compiler does with its
driver library and its plugins.

- cargo builds std as rlibs only, so there is no `libstd.so` in the
  sysroot. A `dylib` crate built with `-C prefer-dynamic` links std's
  rlibs into itself and is the program's one std: every other `dylib` and
  the program depend on it (`DT_NEEDED`) and take their std from it. Two
  objects that each linked std in would be two runtimes, and a panic
  could not cross from one to the other.
- The link is given `libc.so` instead of `libc.a` (mlibc's `shared/`
  before its `lib/`), the dynamic linker
  (`-C link-arg=--dynamic-linker=/lib/ld.so`) and where the objects are
  at run time (`-C link-arg=-rpath=$ORIGIN`).
- The program's entry is mlibc's `crt0.o`, an argument of the program's
  link alone (`cargo rustc -- -C link-arg=.../crt0.o`): std's own
  `_start` is in the shared object then, where no program can start. It
  is weak, so that `crt0.o`'s stands, and reaches `main` through the GOT.
- A loaded object's functions are found with the libc's `dlopen` and
  `dlsym` (std has no API for it), declared `extern "C-unwind"` when a
  panic may leave them.

`user/std-mlibc/dylib` is the pattern and the test (`std_dylib`):
`librt.so` holds std, `libplugin.so` is loaded with `dlopen`, and the
program uses and drops a boxed trait object the plugin made, catches a
panic thrown in it, and runs its code on several threads.

Limits:

- No `proc-macro` crates for the target (they are built for the machine
  the compiler runs on), and no `libstd.so`: see above.
- A `#![no_main]` program must define `main` as C does
  (`extern "C" fn main(argc, argv) -> c_int`): the libc calls it.

Its tests are `/bin/std/mlibc/mix` (`user/std-mlibc`, the `std_mlibc`
test): Rust and C sharing the heap, C's per-thread state on Rust's
threads, Rust on a thread C made, the arguments, `main`'s value and
`atexit`; `/bin/std/mlibc/unwind` (`std_unwind`): panics caught with
their payloads, drops in order, a poisoned lock, panicking threads;
`/bin/std/mlibc/unix` (`std_unix`): the environment, 40 arguments with
one of 3000 bytes, children with pipes, a status and a signal, files
with their links and locks, TCP, UDP and unix sockets over the loopback,
time and named threads; and `/bin/std/mlibc/dylib` (`std_dylib`, above).

### Rust on myos

The compiler itself is a program of the mlibc flavour: the **rustc**
package (`packages/rustc/README.md`) is `rustc` and `cargo` with myos as
their host, built from the pinned nightly's source with the same std.
`get-myos rustc` (on a disk: the compiler's library is 145 MiB), then
`run-myos rustc hello.rs` or `run-myos rustc:cargo build`: programs for
`<arch>-unknown-myos-mlibc`, with build scripts and proc macros, linked
by the llvm app's lld. Under emulation (x86_64, TCG) a small program
compiles in about 15 s and a package with a build script and a proc
macro builds in under 10.

The **rust** package (`packages/rust/README.md`) is the older way: the
official Linux toolchain under the Linux layer, building for the other
flavour.

### Building myos on myos

The kernel, the modules, the Rust programs (`no_std` and `std`) and
newlib build on myos, with that compiler and clang, and myos boots what
it built. The **myos-src** package
(`packages/myos-src/README.md`) is the source the system was built from,
with every crate it is built of (`vendor/`), and `scripts/self-host.sh`
in it runs the build, offline, with no Linux layer:

```sh
mkfs.ext2 /dev/nvme1n1/data && mount /dev/nvme1n1/data /mnt ext2
get-myos -r /mnt/apps myos-src        # with rustc, llvm, make, awk and python
sh /mnt/apps/myos-src/src/myos/scripts/self-host.sh -r /mnt/apps
```

Its main step is `cargo build -p kernel` for the kernel's bare-metal
target on the arch it runs on: the kernel, every module and every
`no_std` program (`init`, `netd`, `http`, ...).

One of them has C in it: `http`, whose TLS is LibreSSL (`user/tls`). The
script compiles LibreSSL first, with the llvm app's clang and the port's
own build script (`ports/libressl/build.sh`, for the one arch, from the
source the package carries), against newlib.

And before it newlib itself, with libgloss (`toolchain/newlib/build.sh`,
for the one arch): newlib's own `configure` and `make all-target-newlib`,
run by oksh, sbase's tools and three apps, GNU make, awk (`config.status`
is an awk program) and python (the script's edits of the installed
headers). The source is the package's (`target/newlib-src`, fetched and
patched where the package was built); `configure` is told what it runs
on (`--build`), which `config.guess` does not know.

`MYOS_SELF_HOST` tells the tree's scripts they run on myos: a port is
current when its outputs exist, since the cache keys of a build machine
need its tools (GNU `sort` and `find`). The rustc app has
`core` and `alloc` for the bare-metal targets; build scripts
(`kernel/build.rs`, which runs the nested builds and the tree's `sh`
scripts) and proc macros are myos programs.

The `std` programs (`user/std`, `/bin/std`) come with their `std`:
Rust's library with myos in it, built from source for
`<arch>-unknown-myos` as on a build machine (`toolchain/std`, `cargo -Z
build-std`). The source is the package's (`target/rust-std-patch`: the
pinned nightly's library as `wire-myos.py` patches it, a Python script,
run where the package is built), the crates `std` is built on are the
ones that library vendors, and the sysroot the programs then link
(`target/myos-sysroot`) is that source, the new rlibs and the rustc
app's own libraries, which build scripts are compiled against. The
`std` of the mlibc flavour is the rustc app's.

The script then packs the initramfs, `target/initramfs-<arch>.cpio.zst`:
the running system's (`/proc/boot/initramfs`) with every module and
program built here in place of its own. The packer is `user/mkinitramfs`,
a program of myos the script builds too; it takes what it writes from the
launcher's packer (`src/initramfs.rs`) and compresses with the kernel's
zstd crate, myos having no `zstd` program. The kernel and that archive
are a boot slot's two files (`docs/install.md`):

```sh
cd /mnt/apps/myos-src/src/myos
cp target/x86_64-unknown-none/debug/kernel /boot/boot/a/kernel
cp target/initramfs-x86_64.cpio.zst /boot/boot/a/initramfs
```

Not built there yet: mlibc, the C userspace and the ports: the new initramfs has them as the system runs them. Nor
the boot disk itself (the ESP, Limine). `linux-compat/self-host.sh` is the older way to
the same kernel, through Alpine's toolchain under the Linux layer
(`docs/linux-compat.md`).

## C userspace (newlib and libgloss)

C programs link against newlib with myos's libgloss
(`toolchain/newlib`). Its syscall adapters are the C both C libraries
share (`toolchain/libmyos`, `docs/mlibc.md`); what has no syscall returns
ENOSYS or EROFS.

libgloss also builds, over the existing calls, what newlib's myos target
lacks:

- constructors and destructors in crt0;
- file times, `reboot`, `settimeofday` and `clock_settime`
  (`CLOCK_MONOTONIC` and its GNU variants do not follow the wall clock);
- `gethostname` and `sethostname` over `/proc/sys/kernel/hostname`;
- `syslog`, with no daemon: each message is one line on the console,
  and on stderr too with `LOG_PERROR`;
- `readv`/`writev`, `vfork`, `daemon`, `getrandom` (from `/dev/urandom`),
  newlib's POSIX `regcomp`, `glob` and `fnmatch`;
- `setitimer`, `getitimer` and `alarm`, from one kernel timer per process
  (`docs/signals.md`);
- POSIX threads on the kernel's threads, with per-thread `errno` and
  stdio (`docs/threads.md`), and an empty `libpthread.a` for
  `-lpthread`.

The netdb service lookups find nothing: there is no services database.

A port's `myos_compat.h` / `myos_stubs.c` holds only what newlib's headers
lack, or what has no honest implementation on myos (rlimits,
`getrusage`). It never redefines what libgloss provides.

### Building

```sh
./toolchain/newlib/build.sh         # fetch newlib 4.4.0, build libc + libgloss/myos
./scripts/build-c-hello.sh          # minimal write() smoke
./ports/sbase/build.sh              # a port (docs/ports.md)
./packages/vim/build.sh             # a package (docs/packages.md)
```

## C userspace (mlibc)

A port opts into [mlibc](https://github.com/managarm/mlibc) 7.0.0 instead
of newlib with `PORT_LIBC=mlibc` (`docs/ports.md`). mlibc runs the same
shared C under its sysdeps (`toolchain/mlibc/sysdeps/myos`), and the image
carries its sysroot at `/lib/mlibc`. The C smokes are built against both
(`/bin/etc/mlibc/*`; `cargo run -- test --tags mlibc`). See
`docs/mlibc.md`.
