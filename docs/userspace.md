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

Programs not in the image (vim, git, `btm` from bottom, ...) are packages:
`get-myos vim` installs one as an app, `run-myos vim` runs it
(`docs/packages.md`). `ps` lists the processes (`docs/proc.md`).

### Terminals and event files

- The framebuffer terminal handles scroll regions, 256/RGB colors and
  UTF-8 with box-drawing, block and braille glyphs. `TERMCAP=/lib/termcap`
  (`ports/termcap`) and termios raw mode serve full-screen TUIs.
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
  (`toolchain/std/build-sysroot.sh`). The port has `std::fs` (no hard links
  or permissions; `canonicalize` through the kernel's `realpath`),
  `std::thread` on native threads with futex locks on `wait_addr` /
  `wake_addr`, and dlmalloc on anonymous `mmap` regions as the heap.
- The kernel has one failure value, so std works out each call's
  `ErrorKind` the way libgloss does.
- Their tests are the `/bin/std/*` programs (`fs`, `thread`, `alloc`) and
  `user/heap`, in the tests module (`docs/testing.md`).

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
