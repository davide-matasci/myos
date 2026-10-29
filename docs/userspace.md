# Userspace

This page covers the native userspace: the syscall interface, the boot from
init to a login shell, and the Rust and C runtimes programs link against
(newlib with libgloss, or mlibc). Read it before you port a program or add
a syscall.

## Syscalls

A syscall takes up to six arguments in registers:

| Arch | Registers |
|------|-----------|
| x86-64 | rdi, rsi, rdx, r10, r8, r9 |
| aarch64 | x0-x5 |
| riscv64 | a0-a5 |

The numbers only grow. A call that was removed leaves its number unused.

### Path calls

Every path call (`kernel/src/user/at.rs`) names its file by a directory fd
and a path relative to it, `*at` style:

- `AT_FDCWD` (-100) is the cwd.
- An absolute path leaves the fd out.
- `AT_EMPTY_PATH` with an empty path is the fd's own file.

| Call | What |
|------|------|
| `openat` | open a file |
| `statat` | 64-bit size, access and modification times, the caller's rights as permission bits, the label owner's uid; `AT_SYMLINK_NOFOLLOW` for a symlink itself |
| `mknodat` | a directory or a FIFO |
| `symlinkat` | |
| `unlinkat` | `AT_REMOVEDIR` for a directory |
| `renameat` | |
| `readlinkat` | |
| `utimensat` | seconds; tmpfs and ext2 keep times, the other filesystems report 0 and refuse |
| `chdirat` | |
| `listdirat` | the names, one per line |
| `execat` | an ELF, or a `#!` script run through its interpreter |
| `statfsat` | syscall 94: how full the file's filesystem is (below) |

`statfsat` gives the blocks in all, free and available, the file nodes in
all and free, and whether the filesystem is read-only. ext2 and FAT give
their own counts, tmpfs the kernel heap's, and the image reads as full.
libc's `statvfs` and `df` use it.

libc's `fstat`, `fstatvfs`, `futimens`, `fchdir`, `fdopendir` and
`fexecve` are these calls on the fd itself. The plain calls (`open`,
`stat`, `chdir`, ...) are these calls on `AT_FDCWD`.

Each call holds the VFS tree lock from resolving its paths to acting on
them, so a directory fd stands for its directory whatever is renamed
meanwhile. The cwd is a directory node too: it follows a rename of its
directory. A directory fd whose directory the caller's namespace cannot
name is a capability: paths resolve beneath it only, with the rights it was
opened with (`docs/security.md`).

`openat` flags:

- `O_CREAT|O_EXCL` creates a new file or fails with `EEXIST`, in one step
  under the tree lock. When several processes race for a name, exactly one
  gets it, and a symlink at the name counts as taken.
- `O_CLOEXEC` makes the fd close at exec.
- `O_NOFOLLOW` refuses a symlink as the last component (`ELOOP`).
- `O_DIRECTORY` refuses anything but a directory (`ENOTDIR`).
- A directory opened to write, create or truncate fails with `EISDIR`.

So a walk of a tree with `openat` cannot be led out of it by a symlink
swapped in.

### File I/O

- `pread(fd, buf, len, offset, flags)` and `pwrite` are also read and
  write. Without `FILE_AT` they work at the file position, which advances.
  With `FILE_AT` they work at `offset` and leave the position as it is
  (`ESPIPE` on a pipe or terminal). A write past the end leaves zeros in
  the gap.
- `ftruncate(fd, size)` cuts a file open for writing, or grows it with
  zeros. tmpfs does it itself, and ext2 through the module's `set_size`
  hook, which gives back the blocks past the end.
- `fdflags(fd, op, flags)` gets or sets an fd's close-on-exec flag
  (`fcntl`'s `F_GETFD`/`F_SETFD`). `dupfd` and `pipe` take the flag too
  (`F_DUPFD_CLOEXEC`, `pipe2(O_CLOEXEC)`).
- Exec closes the fds that have the flag; fork keeps it. A copy made by
  `dupfd` has it only when asked, and `dup2`'s copy never has it.
  `DUP_EXACT` makes `dupfd` a `dup2`, or a `dup3` with the flag.

### File locks

- `flock(fd, op)`: `LOCK_SH`, `LOCK_EX`, `LOCK_UN`, `| LOCK_NB`.
- `lockctl(fd, cmd, range)`: a record lock on a byte range. It gets the
  first conflicting lock, sets one, or sets one and waits. With
  `LOCKCTL_OFD` the lock is the open file description's rather than the
  process's.

These back libc's `flock`, `fcntl`'s `F_GETLK`/`F_SETLK`/`F_SETLKW` and
`lockf`, and Rust's `File::lock` and `try_lock`. `docs/fs.md` has the
lock semantics.

### Other calls

- `exit`, `close`, `fork`, `waitpid`, `brk`, `pipe`, `execname`, `dupfd`,
  `getcwd`, `lseek`, `poll`, `getppid`, `mount`, `umount`, ...
- Memory: `mmap`, `munmap`, `mprotect`, `msync`, `madvise`.
  `MADV_DONTNEED` frees the pages, which read as zero or as their file
  next time, as allocators expect. Other advice is ignored.
- Threads (`docs/threads.md`): `thread_spawn`, `thread_exit`,
  `wait_addr`, `wake_addr`, `gettid`, `set_tp`, `yield`.
- Time: `settimeofday` sets the wall clock until the next boot (the RTC
  keeps its time). `clock_monotonic` gives the nanoseconds since boot, what
  std's `Instant` reads.
- `power`: power off, reboot or halt (`docs/power.md`).
- Security (`docs/security.md`): `setuser` runs as a user, checked against
  the password. `ns` narrows the caller's namespace; libc's `chroot` is a
  namespace of one binding. `policy_load`.

### Removed calls

- The path calls before `*at`: `open`, `exec`, `listdir`, `stat`, `stat2`,
  `stat3`, `chdir`, `mkdir`, `rmdir`, `unlink`, `rename`, `symlink`,
  `readlink`, `mkfifo`, `utimens`, `futimens`, and `chroot`.
- `read` and `write` (numbers 3 and 0): they are `pread` and `pwrite`
  without `FILE_AT`.
- `ioctl` (number 28): a device's state is its `ctl` file (`docs/tty.md`).

## Init and the shell

`user/init` is PID 1. The kernel starts it from the initramfs
(`/bin/custom/init`). It:

1. smoke-tests `fork`, and a fork and exec of `/bin/custom/ok`;
2. runs `mount -a` (`docs/fs.md`);
3. forks netd (`/bin/custom/netd`), the sound server sndd
   (`/bin/custom/sndd`, `docs/sound.md`) and the boot script `/etc/rc`.
   The script runs in the background and ends with
   `[ OK ] boot complete` (`docs/install.md`);
4. forks a getty on the serial port's `/dev/ttyS0` and on `/dev/tty1` to
   `ttyN`, the framebuffer's text screens. `N` comes from `/etc/ttys`
   (`/data/etc/ttys` overrides it).
   Alt+1 to Alt+9 switch screens, and Alt+0 shows the console's screen with
   the boot's and the kernel's lines (`docs/tty.md`);
5. shows tty1's screen, then waits for each getty and starts it again when
   it exits.

The login goes through these steps:

1. getty prompts `login: ` and execs `/bin/ubase/login`.
2. The kernel checks the password (`setuser`; `root` has none). The process
   becomes that user, in its login domain (`root`: `admin`).
3. login execs the shell, `/bin/custom/sh`.

The shell is oksh 7.9 (also `/bin/sh`), with this `PATH`:

```
/bin/sbase:/bin/coreutils:/bin/ubase:/bin/custom:/bin/tcc:/bin/std:/bin/etc
```

### Programs

- `vim` (`/bin/custom/vim`, FEAT_TINY, `packages/vim/README.md`) and `git`
  (`/bin/custom/git`: the porcelain, and clones and fetches over SSH,
  `packages/git/README.md`) are packages. `get-myos vim git` installs them
  as apps, and `run-myos vim` runs one (`docs/packages.md`).
- `ps` lists the processes from `/proc` (`docs/proc.md`).
  `get-myos bottom` and `run-myos bottom` give an interactive `top`,
  `btm`.

### Terminals and event files

- The framebuffer's CSI support includes scroll regions and 256/RGB
  colors. Text is UTF-8, with box-drawing, block and braille glyphs
  (`docs/tty.md`).
- `TERMCAP=/lib/termcap` (`ports/termcap`) and termios raw mode serve
  full-screen TUIs.
- A terminal is a directory with `data` and `ctl` (its termios and window
  size as text). The console is at `/dev/console/`, and the ptys at
  `/dev/pts/N/` from `/dev/pts/clone`. `/proc/self/fd/N` and
  `/proc/self/tty` name them (`docs/tty.md`).

Event counters (eventfd), timers (timerfd), wait sets (epoll) and signal
fds (signalfd) are directories in the same way: `/dev/event/N/`,
`/dev/timer/N/`, `/dev/wait/N/` and `/dev/sig/N/`, each from its `clone`
file. `data` is the fd a program holds, and `ctl` and `status` are its
commands and state as text (`docs/events.md`; libgloss `<sys/eventfd.h>`,
`<sys/timerfd.h>`, `<sys/epoll.h>`, `<sys/signalfd.h>`).

## Rust userspace

- Syscall 9 (`brk`) backs a per-process heap. `user/lib` exposes `brk`,
  `heap_init` and a bump `GlobalAlloc`.
- `user/ok` smoke-tests every boot. `user/heap` is a heavy test suite,
  shipped in the tests module only (`docs/testing.md`).
- `std` programs link the prebuilt sysroot
  (`toolchain/std/build-sysroot.sh`).

The `std` port:

- `std::fs` creates, writes (truncate, append, `create_new`), seeks and
  reads files. It gives an open file's metadata, renames, removes, and
  makes directories and symlinks. `canonicalize` resolves a path to its
  real absolute form through the kernel (`realpath`, syscall 93). There are
  no hard links or permissions.
- The kernel has one failure value, so each call works out its `ErrorKind`
  (`NotFound`, `AlreadyExists`, `IsADirectory`, `ReadOnlyFilesystem`, ...)
  the way libgloss does. Reads and writes tell EIO and EINTR apart
  (`/bin/std/fs` checks them).
- `std::thread` runs native threads. std's futex locks run on
  `wait_addr` / `wake_addr`, and thread-locals sit at the thread pointer
  (`/bin/std/thread`, `docs/threads.md`).
- The heap is dlmalloc, as on Xous, on anonymous `mmap` regions. Freed
  memory is reused, and whole regions go back with `munmap`.
  `/bin/std/alloc` puts 1 GiB through it in a process that stays small.

## C userspace (newlib and libgloss)

C programs link against newlib with myos's libgloss: syscall adapters and
ENOSYS stubs. The adapters are the C both C libraries share
(`toolchain/libmyos`, `docs/mlibc.md`). Implemented libgloss hooks call
real syscalls where they exist; stubs return ENOSYS or EROFS for the rest.

### What libgloss provides

- Its crt0 runs the constructors (`.init_array`,
  `__attribute__((constructor))`) before `main`, and the destructors at
  exit, after the `atexit` handlers (`/bin/etc/ctor_smoke`).
- `stat` fills `st_atime` and `st_mtime` (`st_ctime` = `st_mtime`).
  `utimensat`, `futimens`, `utimes`, `futimes`, `lutimes` and `utime` set
  them.
- `reboot` (`<sys/reboot.h>`) powers off, halts or reboots
  (`docs/power.md`).
- `settimeofday` and `clock_settime` (`CLOCK_REALTIME`) set the wall clock
  (sbase `date MMDDhhmm[[CC]YY]`). `clock_gettime`'s `CLOCK_MONOTONIC`, and
  the GNU `CLOCK_BOOTTIME`, `_RAW` and `_COARSE`, do not follow it: they
  count the kernel's time since boot.
- `gethostname`, `sethostname` and `uname`'s node name read and write
  `/proc/sys/kernel/hostname`.
- `syslog` has no daemon behind it. `openlog`, `syslog` and `vsyslog`
  write each message as one line, `ident[pid]: message`, to the console
  (`/dev/console/data`). With `LOG_PERROR` they write it to stderr as well
  (sbase `logger`).

These have no kernel syscall of their own; libc builds them over the
existing primitives:

- `readv`/`writev` (`<sys/uio.h>`), `vfork`, `daemon`, and `getrandom`
  (from `/dev/urandom`).
- newlib's own POSIX `regcomp`/`regexec`, `glob` and `fnmatch`
  (`libc/posix`). This newlib target leaves them out of libc, so libgloss
  builds them.
- The netdb service lookups find nothing: there is no services database.
- `setitimer` (`ITIMER_REAL`), `getitimer` and `alarm` send `SIGALRM` from
  one kernel timer per process (`docs/signals.md`).
- POSIX threads run on the kernel's threads: `pthread_create`, mutexes,
  condition variables, once, keys. `errno` and stdio are per thread
  (`docs/threads.md`). An empty `libpthread.a` keeps `-lpthread` linking.

A port's `myos_compat.h` / `myos_stubs.c` holds only what newlib's headers
lack, or what has no honest implementation on myos (rlimits,
`getrusage`). It never redefines what libgloss provides.

### Building

```sh
./toolchain/newlib/build.sh         # fetch newlib 4.4.0, build libc + libgloss/myos
./scripts/build-c-hello.sh          # minimal write() smoke
./ports/sbase/build.sh              # 99 sbase utilities, one multicall ELF under /bin/sbase
./ports/ubase/build.sh              # getty, login and df under /bin/ubase/
./ports/oksh/build.sh               # oksh 7.9 as /bin/custom/sh and /bin/sh
./packages/vim/build.sh             # vim FEAT_TINY (a package: get-myos vim)
./ports/zlib/build.sh               # static libz.a for git and get-myos
./ports/libressl/build.sh           # static LibreSSL for curl, lynx, nginx and http
./packages/git/build.sh             # git, SSH remotes (a package: get-myos git)
./packages/tcc/build.sh             # TinyCC, the tcc package (get-myos tcc; -run support)
./ports/ripgrep/build.sh            # ripgrep + PCRE2 as /bin/coreutils/rg
```

## C userspace (mlibc)

A port opts into [mlibc](https://github.com/managarm/mlibc) 7.0.0 instead
of newlib with `PORT_LIBC=mlibc` (`docs/ports.md`). mlibc runs the same
shared C under its sysdeps (`toolchain/mlibc/sysdeps/myos`). It is built
static with meson for the three arches (`./toolchain/mlibc/build.sh`,
`target/mlibc-<arch>`), and the image carries the sysroot at `/lib/mlibc`.
The C smokes are built against it too (`/bin/etc/mlibc/*`), and the boot
tests run them both ways (`cargo run -- test --tags mlibc`). See
`docs/mlibc.md`.
