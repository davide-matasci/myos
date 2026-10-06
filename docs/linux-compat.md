# Linux syscall compatibility layer (optional)

> **Optional, a kernel module.** The layer is the `linux` kernel module
> (`modules/linux`): the kernel itself has no Linux code, only a generic
> *personality* hook (`kernel/src/personality.rs`) the module fills through
> the module ABI. Every image carries the module under `/lib/modules/linux`
> and the `linux` launcher; the `linux_compat` Cargo feature (off by
> default) loads the module at boot and adds the musl test binaries and
> `get-alpine`. Nothing about native myos programs changes when it is
> loaded. myos's own syscall ABI stays the primary interface; this layer is
> an add-on for running some unmodified Linux binaries.

With the module loaded, a Linux binary linked against musl (x86_64, aarch64
or riscv64), static-PIE or dynamically linked, runs on myos when started
through the `linux` launcher:

```sh
cargo run --features linux_compat          # build + boot the x86_64 image
cargo run --features linux_compat -- aarch64   # (or riscv64)
# in the guest:
linux /bin/linux/linux-smoke               # prints LINUX-SMOKE OK
linux /bin/linux/linux-dyn                 # dynamic: prints LINUX-DYN OK
get-alpine jq                              # Alpine Linux packages
linux --root /tmp/alpine jq -n '1+1'
```

In a default build the module is not loaded at boot; `insmod
/lib/modules/linux` loads it (`[ OK ] linux`, listed in `/proc/modules`),
after which `linux PROGRAM` works for any Linux musl binary at hand.

## Alpine Linux packages: `get-alpine`

Nothing from Alpine is in the image: `get-alpine` (`linux-compat/get-alpine.c`,
a native program built with the layer on x86_64, aarch64 and riscv64, all
three of which Alpine has repositories for) downloads packages at run time
with the guest's `curl`:

```sh
get-alpine [-r ROOT] [-u] PACKAGE...       # ROOT defaults to /tmp/alpine
linux --root ROOT PROGRAM [ARG...]
```

1. The `main` and `community` indexes (`APKINDEX.tar.gz`) are downloaded
   once and reduced, streaming, to `ROOT/var/lib/get-alpine/index`: one
   line per package with its repository, version, control checksum,
   dependencies and provides (`so:`, `cmd:`, ...). `-u` refreshes it.
2. Each package and, recursively, its dependencies (`musl` included; a
   `so:libfoo.so.1` dependency resolves to the package providing it) is
   downloaded and checked before anything is written: an `.apk` is three
   concatenated gzip members (signature, control, data); the SHA-1 of the
   control member must match the index's `C:Q1...` and the SHA-256 of the
   data member the `datahash` in the control's `.PKGINFO`. The data tar is
   then unpacked into `ROOT`. Installed packages
   (`ROOT/var/lib/get-alpine/pkgs/`) are skipped. Install scripts are not
   run, and the index signature is not checked (the download is HTTPS).
   A dropped download resumes where it stopped (or starts over from a
   server that cannot resume); three attempts in a row that get no further
   give up.
3. Alpine keeps `/lib` and `/usr/lib` separate, so no symlinks are needed:
   the dynamic linker is `/lib/ld-musl-<arch>.so.1`.

`linux --root ROOT` chroots into `ROOT` (the native `chroot`) before the
exec, with a Linux `PATH`, so the program finds its dynamic linker, shared
objects and data files at their Alpine paths. Before the chroot it
bind-mounts the system's `/dev`, `/proc` and `/net` into `ROOT` (`mount SRC
TARGET bind`; binds last until reboot, and binding a target again replaces
the bind), so the chrooted process sees them like any other directory.

`ALPINE_MIRROR` overrides `https://dl-cdn.alpinelinux.org/alpine` and
`ALPINE_BRANCH` overrides `latest-stable`. `/tmp` is a tmpfs in the kernel
heap, so a root there holds a few small packages and is gone at reboot.
A bigger one goes on a disk, for instance Alpine's Rust compiler (rust,
LLVM and gcc: ~300 MB of downloads, ~600 MB installed) on the scratch disk
of a test boot:

```sh
mkfs.ext2 /dev/nvme1n1 && mount /dev/nvme1n1 /mnt ext2
get-alpine -r /mnt/alpine rust
linux --root /mnt/alpine rustc --version
```

Under emulation that download takes a couple of hours (the mirror drops
long transfers; each one resumes). The same root made on the host is
quicker: `linux-compat/alpine-disk.sh ARCH OUT.img PACKAGE...` builds
get-alpine for the host (`-DALPINE_ARCH`), installs the packages for ARCH
into a directory and makes an ext2 image of it, to attach as a disk.

## Turning it on

| Where | What |
|-------|------|
| `kernel/build.rs` | builds `modules/linux` for the 3 arches like every module (`target/linux-<triple>`) |
| `src/limine_image.rs` | `OPTIONAL_MODULES`: `linux` is in `limine.conf` only with `linux_compat`, in `/lib/modules` always |
| `Cargo.toml` (root) | feature `linux_compat` (not in `default` or `core`) |
| `linux-compat/build-launcher.sh` | the `linux` launcher for each arch, in every image (`build.rs` runs it) |
| `linux-compat/build.sh` | musl, the Linux test binaries and `get-alpine` for each arch (run by `build.rs` when the feature is on) |

Without the module loaded the kernel has no personality registered:
`SYS_LINUX_NEXT_EXEC` (51) fails, so `linux PROGRAM` prints an error, and a
task can never get the Linux personality. Without the feature the image has
no `/bin/linux/` and no `get-alpine`.

## How a process becomes a Linux process

Linux musl binaries and myos newlib binaries are both plain SYSV static-PIE
ELFs, so the kernel does not guess. A process has the **Linux personality**
only if:

1. `linux PROGRAM ...` (`linux-compat/launcher.c`, a native program) calls
   `SYS_LINUX_NEXT_EXEC`, then execs `PROGRAM`: the next successful exec
   starts the image with the Linux personality; or
2. a process that already has it calls Linux `execve`.

`fork` inherits the personality; a native exec clears it. Everything else
(the shell, init, every port) is untouched. Running a native myos binary
under `linux` does not work: it makes myos syscalls that get decoded as
Linux ones. It is killed by its own fault; the kernel stays up.

## Pieces

| Where | What |
|-------|------|
| `kernel/src/personality.rs` | the generic side: which task slots have / are about to get the personality, the registered `PersonalityOps`, the hooks the core calls |
| `modules/linux/src/main.rs` | `module_init`: registers `PersonalityOps` (syscall, deliver, the task hooks); the module's allocator over the kernel heap |
| `modules/linux/src/k.rs` | the kernel as the module sees it: wrappers over `KernelApi` grouped like the old kernel modules (`task`, `user`, `fs`, `signal`, ...) |
| `modules/linux/src/x86_64.rs` | x86_64 syscall numbers, `struct stat`, signal frame, FXSAVE state, `arch_prctl` (the core's thread pointer) |
| `modules/linux/src/aarch64.rs` | signal frame (`fpsimd_context`) |
| `modules/linux/src/riscv64.rs` | signal frame, the sigreturn trampoline code |
| `modules/linux/src/generic.rs` | the `asm-generic` syscall numbers and `struct stat` aarch64 and riscv64 share |
| `modules/linux/src/sys.rs` | the handlers: decode Linux arguments, call the native implementation, return `-errno` |
| `modules/linux/src/signal.rs` | `rt_sigaction` & co., handler delivery, `rt_sigreturn`, the sigreturn trampoline page |
| `modules/linux/src/thread.rs` | `clone` (threads, the fork form and posix_spawn's), `futex`, `set_tid_address`, thread `exit` |
| `modules/linux/src/abi.rs` | errno values, signal-number and sigset translation, `dirent64` layout |
| `modules/linux/src/files.rs` | paths of the fds a Linux process opened (`fstat`, `getdents64`, `fchdir`, `*at`) |
| `linux-compat/launcher.c` | the `linux` command |
| `linux-compat/tests/linux-smoke.c` | Linux-side boot smoke (musl, static) |
| `linux-compat/tests/linux-dyn.c`, `libsmoke*.c` | dynamically linked smoke and its shared objects |
| `linux-compat/get-alpine.c` | the Alpine package fetcher (linked with the zlib port) |

### The personality ABI

A *personality* is a foreign syscall ABI a module provides; one can be
registered (`KernelApi::personality_register`, ABI 13). The kernel keeps
two bits per task slot, *active* and *pending*, and calls the module's
`PersonalityOps` at these points (all generic, nothing Linux-specific in
the kernel):

- `syscall_dispatch`: an active task's syscalls go to `ops.syscall(nr, a0,
  a1, a2, regs)` instead of the native table (then the usual signal
  handling at syscall exit); `regs` is the saved user register block, so
  the module reads further arguments and sets the result the arch way;
- spawn / fork / thread creation / exec (`task/lifecycle.rs`,
  `task/thread.rs`): the kernel resets, copies or applies the bits and
  calls `ops.on_spawn / on_fork / on_thread / on_exec` so the module can do
  the same with its own per-slot state (fd paths, the trampoline, the
  `clear_tid` word). The thread pointer and the FP/SIMD registers are
  switched for every user task by the core (`task/tp.rs`, `task/fpu.rs`);
- signal delivery (`signal.rs`): a caught signal of an active task goes to
  `ops.deliver(regs, &SignalDelivery, &mut ret)` (the decided handler,
  trampoline, return context and the native mask the handler's return
  restores; on x86_64 `arch` carries the user code segment), which writes
  the foreign frame or fails (the kernel then kills the task with
  `SIGSEGV`);
- riscv64 user entry (`arch/riscv64/user.rs`, `user_sstatus`): with
  `PERSONALITY_FPU_ON` in `ops.flags`, active tasks run with the FPU on
  (`sstatus.FS`); native programs are soft-float;
- exec (`user/syscall.rs`): with the personality pending or active, the
  new image gets the SysV auxv entries (`AT_PHDR`, `AT_PHNUM`, `AT_ENTRY`,
  `AT_BASE`, ..., and the ids, all 0: without `AT_UID`/`AT_EUID`/`AT_GID`/
  `AT_EGID` musl runs as setuid and ignores `LD_LIBRARY_PATH`) and its
  `PT_INTERP` dynamic linker is mapped (see below).
  `KernelApi::personality_exec` is the exec that keeps the personality
  (Linux `execve`); `SYS_LINUX_NEXT_EXEC` sets the pending bit for the
  launcher.

The module's other needs are plain `KernelApi` services added with ABI 13:
`native_syscall` (a native syscall from kernel mode, e.g. `read`, `brk`,
`waitpid`), `copy_from_user`, the path / VFS calls (`path_resolve`,
`vfs_stat`, `vfs_listdir`, ...), fd calls (`fd_pread`, `fd_kind`,
`fd_dup2`, `pipe_open`, ...), `mmap`, the signal table (`signal_get_action`
/ `signal_set_action`, masks, `signal_kill`, `signal_sigsuspend`, ...),
`fpu_save` / `fpu_restore`, the thread pointer, `thread_spawn_from` (a
thread resuming like the caller of a syscall on a new stack, for `clone`),
`wait_addr` / `wake_addr` (for `futex`), `task_sleep_until`, `wall_time_us`
and `rng_fill`; ABI 25 added `vfs_set_times`, ABI 27 `thread_place` (a
`clone`d thread's own CPU, once its ids are stored). The module uses `alloc` (`Vec`, `String`) through the
kernel heap (`KernelApi::alloc` / `dealloc`).

## Supported syscalls

Numbers differ per arch (x86_64 has its own table; aarch64 and riscv64 share
the `asm-generic` one, which has only the `*at`, `clone`, `ppoll` and `dup3`
forms of the legacy calls). The set:

Files: `read`, `write`, `readv`, `writev`, `open`, `openat`, `close`, `stat`,
`lstat`, `fstat`, `newfstatat`, `lseek`, `getdents64`, `ioctl` (the tty
requests from the terminal's ctl file, `docs/tty.md`; `FIONBIO` on sockets;
the console keymap and module devices natively), `access`,
`faccessat`, `pipe`, `pipe2`, `eventfd(2)`, `dup`, `dup2`, `dup3`, `fcntl`
(dup, fd flags, record locks), `flock`, `truncate`, `ftruncate`, `fsync`,
`fdatasync`, `chmod`, `chown` (and their `f`, `l`, `at` forms), `getcwd`,
`chdir`, `fchdir`, `mkdir(at)`, `rmdir`, `unlink(at)`, `rename(at/at2)`,
`symlink(at)`, `readlink(at)`, `utimensat`, `utimes`, `poll`, `umask`.

Memory: `brk`, `mmap` (anonymous, and private file mappings), `munmap`,
`mprotect`, `madvise` (`MADV_DONTNEED` drops the pages, which read as
zero or as their file next time, as allocators such as rustc's Scudo
expect; other advice is ignored). Files also: `pread64`, `pwrite64`,
`pwritev(2)`.

Processes: `fork`, `vfork` (as fork), `clone` (see Threads), `execve` (a
`#!` script runs its interpreter, with the line's one optional argument;
a file that is neither an ELF nor a script fails with `ENOEXEC`),
`exit` (the thread), `exit_group`, `wait4`, `kill`, `tkill`, `tgkill`,
`getpid`, `gettid`, `getppid`, `getpgid`, `setpgid`, `getpgrp`, `getsid`,
`setsid`, `uname` (the node name is the kernel's host name,
`/proc/sys/kernel/hostname`), `sethostname`, `reboot` (the native
`power`, `docs/power.md`; turning Ctrl-Alt-Del on or off does nothing),
`arch_prctl`, `set_tid_address`, `set_robust_list`,
`prlimit64`, `getrlimit`, `get/set uid/gid` (uid and gid 0 whoever runs the
program: what it may do is the security policy's, `docs/security.md`),
`sched_yield`.

Threads: `clone` with `CLONE_THREAD` (and `CLONE_VM`, `CLONE_FS`,
`CLONE_FILES`, `CLONE_SIGHAND`; `CLONE_SETTLS`, `CLONE_PARENT_SETTID`,
`CLONE_CHILD_SETTID`, `CLONE_CHILD_CLEARTID`) starts a native thread
(`docs/threads.md`) that resumes like a forked child on its new stack;
`futex` `WAIT`/`WAKE` (and the `_BITSET` forms, the bitset taken as "all";
`REQUEUE`/`CMP_REQUEUE` as a wake of every waiter) maps onto the core's
`wait_addr`/`wake_addr`; a thread's `exit` clears and wakes its
`CLEAR_TID` word, which is what `pthread_join` waits on. `clone` without
`CLONE_THREAD` is the fork form, or `posix_spawn`'s (`CLONE_VM |
CLONE_VFORK` with a stack, what Rust's `Command` uses through musl): a fork
whose child starts on that stack (the core's `fork_from`). Its memory is a
copy rather than shared, which posix_spawn does not notice: its child
reports a failed exec through a pipe the parent waits on.

Signals: `rt_sigaction` (handlers with `SA_SIGINFO`, `SA_RESTART`,
`SA_NODEFER`, `SA_RESETHAND`, `sa_mask`), `rt_sigreturn`, `rt_sigprocmask`,
`rt_sigpending`, `rt_sigsuspend`, `rt_sigtimedwait`, `sigaltstack` (no
alternate stacks). Signal numbers and masks are translated between Linux and
the native (newlib) numbering, also in `wait4` statuses.

Sockets: `socket` (`AF_INET` stream and datagram), `connect`, `sendto`,
`recvfrom`, `sendmsg`, `recvmsg`, `shutdown`, `getsockname`, `getpeername`,
`getsockopt` (`SO_ERROR`, `SO_TYPE`), `setsockopt` and `bind` (accepted,
ignored); `read`/`write`, `poll`, `fstat` and `ioctl(FIONBIO)` work on them
too. See Sockets below.

Time and misc: `clock_gettime`, `gettimeofday`, `time`, `nanosleep`,
`clock_nanosleep`, `getrandom`; `poll` (x86_64) and `ppoll`;
`sched_getaffinity` (every online CPU, so cargo and rustc run their jobs in
parallel) and `sched_setaffinity` (accepted; the core places tasks).

Anything else returns `ENOSYS`.

## Sockets

A Linux socket is a conversation of the native `/net` (`docs/sockets-curl.md`;
`modules/linux/src/net.rs`), whose fd is the conversation's `data` file:
reads, writes, `dup`, `fork` and `close` are ordinary fd operations, and the
last close hangs the conversation up. `socket` reads `/net/{tcp,udp}/clone`,
`connect` writes `connect a.b.c.d!port` to its `ctl`, and readiness comes from
its `status` and the bytes waiting in `data`. The `/net` files never block,
so a blocking read or connect sleeps until netd's next reply wakes the
pollers (a write retries while netd's request ring is full, or a TCP
conversation has no send room left: 8 KiB queued in netd). A datagram
socket sends to the last address it was given, and reads one datagram at a
time. A chrooted process reaches `/net` through the bind `linux --root` sets
up, and `get-alpine` gives a new root an `/etc/resolv.conf` naming the
resolver the system uses (QEMU's `10.0.2.3`) for musl.

## Dynamic linking

A dynamically linked program names its interpreter in `PT_INTERP` (musl:
`/lib/ld-musl-<arch>.so.1`, which is `libc.so` itself). For a Linux exec of
such a program the kernel:

1. reads the interpreter (`exec_interp` in `user/syscall.rs`) before
   replacing the current image, so a missing one fails the exec cleanly;
2. maps the program as usual but **without applying its relocations**
   (`elf::realize_as(.., relocate: false)`): they refer to symbols in shared
   objects, which the dynamic linker resolves;
3. maps the interpreter, also unrelocated (it relocates itself), at the start
   of the new image's `mmap` window, with per-segment protections, and
   records it as `mmap` regions (so fork copies it and exit frees it);
4. starts the interpreter, with `AT_BASE` = its load address and
   `AT_PHDR` / `AT_ENTRY` naming the program.

The dynamic linker then loads the `DT_NEEDED` objects (and later `dlopen`
ones) from `/lib`, `/usr/local/lib`, `/usr/lib` with `open`, `read`,
`pread64` and `mmap` of the file, and maps each segment over the span it
reserved first. That needed core `mmap` work, which native programs share:

- file-backed `MAP_PRIVATE` mappings (the pages are a private copy of the
  file, never written back; `MAP_SHARED` file mappings are refused);
- demand paging: a mapping takes no memory until it is used. The first
  touch of a page, a page fault from userspace or a kernel copy into a user
  buffer, gives it a frame, zeroed or read from the file
  (`user::fault_in`). A page mapped without write permission (code,
  constant data) is the page cache's frame for it, shared by every process
  that maps it (`fs/pagecache.rs`); a writable one is a private copy of it.
  rustc reserves 256 MiB for its allocator and maps
  some 250 MiB of libraries, and `rustc --version` touches about 30 MiB of
  it. A file changed or deleted while mapped gives its new contents (or
  zeros) to the pages not touched yet;
- `MAP_FIXED` replaces whatever is mapped in its range;
- `munmap` of any range in the `mmap` window (holes included) and
  `mprotect` of part of a mapping split the mapping;
- free address space is reused (first fit) instead of only growing;
- a larger window (4 GiB on x86_64, 960 MiB on aarch64 / riscv64, whose
  user address spaces span 1 GiB) and 4096 mappings per process.

## Signal handlers

Delivery reuses the native decision path (masks, `SA_RESTART`, `EINTR`,
`sigsuspend`; see `docs/signals.md`): only the frame differs. For a Linux
task the kernel writes a Linux `rt_sigframe` below the interrupted stack
pointer, with `siginfo_t` and a `ucontext` holding the interrupted
registers, blocked mask and FP/SIMD state (x86_64: FXSAVE image; aarch64:
`fpsimd_context`; riscv64: the D state), and enters the handler as
`handler(sig, &info, &uc)` returning to the restorer, which calls
`rt_sigreturn`. musl passes `SA_RESTORER` on x86_64 and aarch64; riscv64 has
no `sa_restorer` (Linux uses its vDSO there), so the kernel maps a one-page
trampoline into the process on the first such `rt_sigaction`.

On x86_64 the callee-saved registers (rbx, rbp, r12-r15) are reported as 0
in `uc_mcontext` and not restored from it: the handler preserves them, and
the kernel does not keep a per-task copy at syscall entry.

## Building myos in myos

`linux-compat/self-host.sh DIR [REV]` (in the image as `/lib/self-host.sh`)
builds the x86_64 kernel inside myos with Alpine's Rust toolchain:

```sh
mkfs.ext2 /dev/nvme1n1 && mount /dev/nvme1n1 /mnt ext2
sh /lib/self-host.sh /mnt           # REV: the branch to build (master)
```

It installs Alpine's rust, cargo, rust-src, lld, clang, bash, busybox and
git into `DIR/alpine` with `get-alpine`, clones (or updates) the source,
builds `core`, `alloc` and `compiler_builtins` once for
`x86_64-unknown-none` into Alpine's `rustlib` (Alpine ships them for its
own target only; the kernel's `build.rs` builds every module and user
program in a target directory of its own), then runs
`cargo build -p kernel` under `linux --root`. Each step is skipped when its
result is already there. The scratch disk needs a few GiB.

Not built there yet, copied from the running system instead: the C and
`std` programs the kernel embeds (the std demos, c-hello, oksh, getty,
login) and `http`, whose TLS library is mbedtls over newlib
(`MYOS_PREBUILT=http` has `kernel/build.rs` take it as it is). Build
scripts are linked with clang: Alpine's gcc is not position-independent,
which the Linux layer requires.

The kernel it builds boots: with it in the image instead of the host's,
`cargo run -- test-mini` passes. Under TCG a first run takes hours:
`core` and `alloc` about 25 minutes, each module or user program one to
three, the kernel crate itself an hour. Known gaps:

- `http` (mbedtls and newlib: autotools, `make`, `python3`) and the other
  C programs are not built in myos;
- non-PIE Linux programs (Alpine's gcc) do not run (see Limits).

## Limits

- PIE musl binaries only (static-PIE or dynamically linked), no glibc ones
  (not tried: glibc needs more syscalls), and no non-PIE `ET_EXEC` images:
  the loader places every image in the fixed
  per-arch user window (non-PIE binaries are linked at 0x400000 / 0x10000,
  which needs a redesign of the user address-space layout; so e.g. Alpine's
  prebuilt `busybox.static` does not load).
- Signals act at syscall exit, as for native programs: a task looping in
  user mode is not interrupted until its next syscall (only `SIGKILL` acts
  on the interrupt return). No alternate signal stacks, no real-time signal
  queueing.
- No other `clone` with `CLONE_VM` but not `CLONE_THREAD` than
  posix_spawn's, and no shared file mappings (`MAP_SHARED`).
- Close-on-exec (`O_CLOEXEC`, `FD_CLOEXEC`, `FIOCLEX`) and `O_NONBLOCK`
  outside sockets are flags the layer keeps per fd (`files.rs`): a
  successful exec closes the close-on-exec fds (the `on_exec` hook), and a
  non-blocking pipe end that would block gives `EAGAIN` (from the core's
  pipe readiness, as libgloss does for native programs).
- An eventfd is a pipe (its write end kept aside): a non-zero write wakes
  a reader, a read returns how many writes it collected. No semaphore mode
  and no initial value.
- `socketpair(AF_UNIX, SOCK_STREAM | SOCK_SEQPACKET)` is a `/net/unix`
  pair (`docs/sockets-unix.md`), its ends kept like TCP sockets (Rust's
  `Command` reports a failed exec through one). Stream semantics for
  both: no message boundaries, and no fd passing.
- File locks are the native ones (`README.md`, "File locks"): `flock`,
  `fcntl`'s record locks (`F_SETLK`, `F_SETLKW`, `F_GETLK`) and the open
  file description's (`F_OFD_SETLK`, ...), so cargo, SQLite and git
  guard against each other as on Linux. `ftruncate` and `O_CREAT|O_EXCL`
  are native too (any length; the name checked and created in one step).
  `O_NOFOLLOW` and `O_DIRECTORY` are kept (`ELOOP`, `ENOTDIR`). `link` and
  `linkat` fail with `EPERM`: myos has no hard links.
  `pwrite` past the end of a file leaves zeros in the gap.
- `chmod`, `chown` and `fsync` succeed and do nothing: what a program may
  do to a file is the security policy's (`docs/security.md`), and ext2
  writes a file back when its last fd closes. `stat`'s permission bits are
  the owner's only and say what the calling program may do; the layer's
  file calls are checked like the native ones.
- File times: `stat` reports a file's access and modification times in
  whole seconds (`st_ctime` is the modification time) where the filesystem
  keeps them, tmpfs and ext2; the read-only and device filesystems (`/bin`,
  `/dev`, `/proc`, ...) and FAT report 0. `utimensat`, `futimens` and
  `utimes` set them there (`UTIME_NOW`, `UTIME_OMIT`; nanoseconds are
  dropped) and fail with `EROFS` elsewhere.
- Sockets: IPv4 clients and `socketpair` only (no `listen`/`accept`, no
  IPv6, no other Unix sockets); no half-close (`shutdown` hangs up only for
  `SHUT_RDWR`); the local address is reported as `0.0.0.0:0`; options are
  ignored; `poll` reports a connected socket writable even with no send
  room left (a nonblocking write then says `EAGAIN`).
- The native limits apply:
  - exec: up to 1024 arguments and 1024 environment strings, at most
    128 KiB together. A dynamically linked, position-independent program
    (most of Alpine's) is mapped from its file like a shared object: its
    dynamic linker is loaded as the image, the program's segments become
    mmap regions paged in on first touch, so only the mmap window bounds
    its size (Alpine's cargo is 23 MB). Any other program is read whole: a
    file of at most 16 MiB from a writable filesystem (the initramfs has
    no limit) whose loaded image spans at most 1152 pages (4.5 MiB);
  - a per-process `mmap` window of 4 GiB (x86_64) / 960 MiB (aarch64,
    riscv64) with at most 4096 mappings; adjacent mappings with the same
    protection and backing are merged (musl's malloc makes hundreds of
    small neighbouring ones: jq peaks at 188; rustc's Scudo allocator
    needs more than 256 to compile `core`). At most 64 distinct files
    are mapped at once; a file mapping past that is read in whole when it
    is made;
  - a 16 MiB `brk` heap;
  - 64 fds per process, 512 open file descriptions in the system, 64 tasks
    in total, 64 pipes in the system (and 24 named FIFOs), each with a
    4 KiB buffer;
  - `/tmp` (tmpfs) files of at most 16 MiB each, all of them in the
    kernel heap (a quarter of the memory, 64 MiB to 1 GiB: 256 MiB in the
    1 GiB CI guests).

## Testing

The boot tests (`docs/testing.md`, `user/tests/kernel.sh`) run
`linux /bin/linux/linux-smoke` and `linux /bin/linux/linux-dyn` on every
arch when the image was built with `--features linux_compat`, and expect
`LINUX-SMOKE OK` (files, directories, mmap, fork/execve/wait4, pipes, Linux
signal numbers, handlers, masks, `sigwait`, `EINTR` and `SA_RESTART`,
pthreads: a mutex, a condition variable, thread-local storage, thread ids,
join, and `exit` from a thread ending the process) and `LINUX-DYN OK` (a
call, shared data, a relocated function pointer and a thread-local in
`libsmoke.so`, `printf` from `libc.so`, `dlopen`/`dlsym` of `libsmoke2.so`).
The test is plain musl C, so it can also be run on a Linux host for
reference. The full list then runs
`get-alpine jq && linux --root /tmp/alpine jq -Rrn '...' /proc/mounts`,
which counts the binds of `/dev`, `/proc` and `/net` it sees in the root and
expects `ALPINE-JQ 6`, then installs Python into the same root (python3 and
its 19 dependencies, ~45 MB) and runs
`python3 -c 'import json,sqlite3;print("PYTHON",json.loads("[42]")[0])'`
(the standard library and two C extension modules), expecting `PYTHON 42`,
and fetches `http://example.com/` with `urllib` (DNS over UDP, then TCP),
expecting `HTTP 200`. Last, it mounts the disk the launcher made on the
host with `linux-compat/alpine-disk.sh ARCH target/alpine-rust-ARCH.img
rust` (kept in `target/`: remove it for newer packages) and attached as
`/dev/nvme2n1` (writes go to a QEMU snapshot), and runs `rustc --version`
from it, expecting `rustc 1.`. They need the Alpine mirror and
`example.com` to be reachable.

Without the feature (the quick list, the normal PR CI), the test instead
runs `insmod /lib/modules/linux` and expects `[ OK ] linux` and the module
in `/proc/modules`: the module is built and loadable in every build.

CI builds the musl pieces in every run (`linux-compat` in
`ci-ports.yml`, cached in the GHCR registry by `scripts/ci-registry.sh`
under a hash of `linux-compat/`, newlib and zlib) and packs them into
`ci-build.tar`. The full boot (daily, or `full_boot` on dispatch;
`MYOS_CI_FEATURES=linux_compat`) builds all four disk images with the
layer, so bios, uefi, aarch64 and riscv64 run the tests above. On master
the **iso** job builds the x86_64 hybrid ISO with `--features linux_compat`,
so the downloadable ISO always carries the layer (module loaded at boot,
`linux-smoke`, `linux-dyn`, `get-alpine`). There are no separate Linux jobs.

`linux-compat/build.sh` builds musl (static and shared) with clang for each
target (and skips itself when its outputs match its stamp,
`target/.myos-linux-compat-version`). On aarch64/riscv64 musl's `long double` is 128-bit and needs
compiler-rt's quad-float builtins: the script fetches those sources (pinned
LLVM tag) and links them into `libc.so`. The static smoke test is not linked
against them, so it (and the launcher) avoid `printf`.
