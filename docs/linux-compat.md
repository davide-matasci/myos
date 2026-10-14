# Linux syscall compatibility layer (optional)

> **Optional, a kernel module.** The layer is the `linux` kernel module
> (`modules/linux`): the kernel itself has no Linux code, only a generic
> *personality* hook (`kernel/src/personality.rs`) the module fills through
> the module ABI. Every image carries the module under `/lib/modules/linux`
> and the `linux` launcher; the `linux_compat` Cargo feature (off by
> default) loads the module at boot and adds the musl test binaries.
> `get-alpine`, which installs Alpine Linux packages (with Alpine's own
> `apk`, run under the layer), is a myos package (`get-myos get-alpine`).
> Nothing about native myos programs changes when it is loaded. myos's own syscall ABI stays the primary interface; this layer is
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
get-myos get-alpine                        # the Alpine package installer, a package
run-myos get-alpine jq                     # Alpine Linux packages, into /data/alpine
linux --root /data/alpine jq -n '1+1'      # (/tmp/alpine without a /data)
linux --root /data/alpine apk add python3  # apk itself, once the root has it
```

In a default build the kernel does not load the module at boot; the boot
script's default `/data/etc/rc.local` does (`docs/install.md`), with
`insmod /lib/modules/linux` (`[ OK ] linux`, listed in `/proc/modules`),
after which `linux PROGRAM` works for any Linux musl binary at hand, and
the lines above from `get-myos get-alpine` on work the same: nothing of
the layer but the module and the launcher is needed to run Alpine's
packages.

## Alpine Linux packages: `get-alpine`

Nothing from Alpine is in the image: `get-alpine`
(`packages/get-alpine/get-alpine.c`, a native program for x86_64, aarch64
and riscv64, all three of which Alpine has repositories for) bootstraps
a Linux root at run time and has Alpine's own package manager, `apk`,
install the packages into it. It is a package (`docs/packages.md`), not
in the image: `get-myos get-alpine` installs it as an app, `run-myos
get-alpine` runs it, with the layer loaded (apk runs under it).

```sh
run-myos get-alpine [-r ROOT] [-u] [-n] [PACKAGE...]   # ROOT: /data/alpine or /tmp/alpine
linux --root ROOT apk add|del|upgrade|info ...      # apk itself, afterwards (no PACKAGE: just the bootstrap)
linux --root ROOT PROGRAM [ARG...]
```

1. **Bootstrap** (once per root, natively, with the guest's `curl`): the
   `main` and `community` indexes (`APKINDEX.tar.gz`) are downloaded and
   reduced, streaming, to `ROOT/.get-alpine/index` (one line per package
   with its repository, version, control checksum, dependencies and
   provides: `so:`, `cmd:`, ...), and what apk needs to run is fetched
   through it: `apk-tools`, `alpine-keys` (the keys apk checks the
   repositories with), `busybox` and `busybox-binsh` (`/bin/sh` and the
   tools install scripts use), `alpine-baselayout` (the users and
   directories they expect) and `ca-certificates-bundle` (the trust
   store of every TLS client, which python3, curl or wget alone do not
   pull in), with their dependencies (`musl`, `libapk`, `libcrypto3`,
   ...). Each is checked before anything is written: an `.apk` is three
   concatenated gzip members (signature, control, data); the SHA-1 of the
   control member must match the index's `C:Q1...` and the SHA-256 of the
   data member the `datahash` in the control's `.PKGINFO`. The data tar
   is then unpacked into `ROOT` and the package recorded in
   `ROOT/.get-alpine/pkgs/`. (The index signature is not checked here,
   the download is HTTPS; apk checks it next, with the keys just
   unpacked.) The root also gets `/etc/apk/repositories` (the mirror and
   branch of this run) and an `/etc/resolv.conf` (below). A dropped
   download resumes where it stopped (or starts over from a server that
   cannot resume); three attempts in a row that get no further give up.
2. **apk**: `linux --root ROOT apk add --initdb --no-progress BASE...
   PACKAGE...` (`BASE` the bootstrap set, so apk's world lists it from
   the start; `--initdb` makes the database on the first run and is
   nothing afterwards) resolves the dependencies, downloads the packages
   (with musl's networking over the layer), checks the repository
   signatures and the package checksums, unpacks them, runs their
   install scripts and triggers (busybox's applet links, GSettings
   schemas, gdk-pixbuf loaders, the MIME database, users and groups:
   what the by-hand steps of a scriptless unpack were), and records them
   in `ROOT/lib/apk/db` and `ROOT/etc/apk/world`. On the first run it
   installs the bootstrap set once more, over the unpacked files, so the
   database owns it too. An installed package costs nothing; `-u`
   (`--update-cache`) refreshes the indexes first. From then on
   `linux --root ROOT apk ...` is the whole package manager: `add`,
   `del`, `upgrade`, `info`, `fix`.
3. `-n` leaves apk out: the packages asked for are fetched, checked and
   unpacked natively, as the bootstrap set is, without their install
   scripts and unknown to apk's database. The layer need not be loaded;
   `linux-compat/alpine-disk.sh` uses it on the host, which has no layer
   to run apk under.
4. Alpine keeps `/lib` and `/usr/lib` separate, so no symlinks are needed:
   the dynamic linker is `/lib/ld-musl-<arch>.so.1`.

`linux --root ROOT` chroots into `ROOT` (the native `chroot`) before the
exec, with a Linux `PATH`, so the program finds its dynamic linker, shared
objects and data files at their Alpine paths. Before the chroot it
bind-mounts the system's `/dev`, `/proc` and `/net` into `ROOT` (`mount SRC
TARGET bind`; binds last until reboot, and binding a target again replaces
the bind), so the chrooted process sees them like any other directory.

`ALPINE_MIRROR` overrides `https://dl-cdn.alpinelinux.org/alpine` and
`ALPINE_BRANCH` overrides `latest-stable`. The default root is
`/data/alpine` when the boot disk's data partition is mounted at `/data`
(`docs/install.md`), kept across reboots and upgrades, else `/tmp/alpine`:
`/tmp` is a tmpfs in the kernel heap, so a root there holds a few small
packages and is gone at reboot. `etc/policy` labels `/data/alpine`
`sys.pkg`, as `get-myos`'s apps: a user's shell reads and runs what is
there, and the administrator installs it. A root too big for the data
partition goes on another disk, for instance Alpine's Rust compiler (rust,
LLVM and gcc: ~300 MB of downloads, ~600 MB installed) on the scratch disk
of a test boot:

```sh
mkfs.ext2 /dev/nvme1n1/data && mount /dev/nvme1n1/data /mnt ext2
run-myos get-alpine -r /mnt/alpine rust
linux --root /mnt/alpine rustc --version
```

Under emulation that download takes a couple of hours (the mirror drops
long transfers; a package whose transfer apk loses is installed by the
next run). The same root made on the host is
quicker: `linux-compat/alpine-disk.sh ARCH OUT.img PACKAGE...` builds
get-alpine for the host (`-DALPINE_ARCH`), unpacks the packages for ARCH
into a directory (`-n`: without their install scripts, and apk's database
in that root is empty) and makes an ext2 image of it, to attach as a disk.

## Turning it on

| Where | What |
|-------|------|
| `kernel/build.rs` | builds `modules/linux` for the 3 arches like every module (`target/linux-<triple>`) |
| `src/limine_image.rs` | `OPTIONAL_MODULES`: `linux` is in `/lib/modules/boot.list` only with `linux_compat`, in `/lib/modules` always |
| `Cargo.toml` (root) | feature `linux_compat` (not in `default` or `core`) |
| `linux-compat/build-launcher.sh` | the `linux` launcher for each arch, in every image (`build.rs` runs it) |
| `linux-compat/build.sh` | musl and the Linux test binaries for each arch (run by `build.rs` when the feature is on) |

Without the module loaded the kernel has no personality registered:
`SYS_LINUX_NEXT_EXEC` (51) fails, so `linux PROGRAM` prints an error, and a
task can never get the Linux personality. Without the feature the image has
no `/bin/linux/`.

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
| `packages/get-alpine/get-alpine.c` | the Alpine package installer: bootstraps apk, which installs (a package, linked with the zlib port) |

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
  `clear_tid` word). `on_spawn`, `on_fork` and `on_thread` run with the
  core's task lock held and interrupts off, so a lock the module takes in
  them is never held while calling into the core (`current_pid` takes
  that lock: `files.rs` reads the pid before locking its tables). The
  thread pointer and the FP/SIMD registers are switched for every user
  task by the core (`task/tp.rs`, `task/fpu.rs`);
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
`lstat`, `fstat`, `newfstatat`, `statfs`, `fstatfs` (no filesystem magic in
`f_type`), `lseek`, `getdents64`, `ioctl` (the tty
requests from the terminal's ctl file, `docs/tty.md`; `FIONBIO` on sockets;
the console keymap and module devices natively), `access`,
`faccessat`, `pipe`, `pipe2`, `eventfd(2)`, `timerfd_create`,
`timerfd_settime`, `timerfd_gettime`, `epoll_create(1)`, `epoll_ctl`,
`epoll_wait`, `epoll_pwait(2)`, `signalfd(4)`, `dup`, `dup2`, `dup3`,
`fcntl`
(dup, fd flags, record locks), `flock`, `truncate`, `ftruncate`, `fsync`,
`fdatasync`, `chmod`, `chown` (and their `f`, `l`, `at` forms), `getcwd`,
`chdir`, `fchdir`, `mkdir(at)`, `rmdir`, `unlink(at)`, `rename(at/at2)`,
`symlink(at)`, `readlink(at)`, `utimensat`, `utimes`, `poll`, `umask`.

Memory: `brk`, `mmap` (anonymous, shared anonymous, private and shared
file mappings), `munmap`, `mprotect`, `msync`, `madvise` (`MADV_DONTNEED` drops the pages, which read as
zero or as their file next time, as allocators such as rustc's Scudo
expect; other advice is ignored), `mremap` (a mapping shrinks in place;
it cannot grow, `ENOMEM`, and musl's `realloc` then allocates anew and
copies: issue #421; a range that is not mapped is `EFAULT`, which ends
musl's `pthread_getattr_np` walk down the main thread's stack). Files also: `pread64`, `pwrite64`, `pwritev(2)`, `fallocate`
(mode 0, what `posix_fallocate` asks: the file grows to the range's end,
its blocks not reserved ahead; other modes are `EOPNOTSUPP`),
`readahead` (a hint, done).

Processes: `fork`, `vfork` (as fork), `clone` (see Threads), `execve` (a
`#!` script runs its interpreter, with the line's one optional argument,
the kernel's way: `exec` on the script and its `exec` rule apply,
`docs/security.md`; a file that is neither an ELF nor a script fails with
`ENOEXEC`),
`exit` (the thread), `exit_group`, `wait4`, `waitid` (`P_ALL`, `P_PID`;
`WNOHANG`, `WNOWAIT`; exits only), `kill`, `tkill`, `tgkill`,
`getpid`, `gettid`, `getppid`, `getpgid`, `setpgid`, `getpgrp`, `getsid`,
`setsid`, `uname` (the node name is the kernel's host name,
`/proc/sys/kernel/hostname`), `sysinfo` (uptime and memory from
`/proc/meminfo`; no swap, load or process count), `sethostname`, `reboot` (the native
`power`, `docs/power.md`; turning Ctrl-Alt-Del on or off does nothing),
`arch_prctl`, `set_tid_address`, `set_robust_list`,
`prlimit64`, `getrlimit`, `get/set uid/gid` and `getresuid`/`getresgid`
(uid and gid 0 whoever runs the program: what it may do is the security
policy's, `docs/security.md`), `getpriority` (nice 0) and `setpriority`
(accepted; the core schedules), `prctl` (`PR_SET_NAME` and `PR_SET_VMA`
accepted and not kept, `PR_GET_DUMPABLE` set, `PR_SET_PDEATHSIG` and
`PR_SET_NO_NEW_PRIVS` accepted and not done; any other option, seccomp
included, `EINVAL`), `sched_yield`. No namespaces: `unshare` of anything
and `clone` with a `CLONE_NEW*` flag are `EINVAL` (bubblewrap says so and
glycin, WebKitGTK's image loader, runs its loaders without a sandbox);
`capget` reports no capabilities and `capset` grants none;
`/proc/sys/kernel/overflowuid` and `overflowgid` read 65534.

Threads: `clone` with `CLONE_THREAD` (and `CLONE_VM`, `CLONE_FS`,
`CLONE_FILES`, `CLONE_SIGHAND`; `CLONE_SETTLS`, `CLONE_PARENT_SETTID`,
`CLONE_CHILD_SETTID`, `CLONE_CHILD_CLEARTID`) starts a native thread
(`docs/threads.md`) that resumes like a forked child on its new stack;
`futex` `WAIT`/`WAKE` (and the `_BITSET` forms, the bitset taken as "all";
`REQUEUE`/`CMP_REQUEUE` as a wake of every waiter) maps onto the core's
`wait_addr`/`wake_addr`; a thread's `exit` clears and wakes its
`CLEAR_TID` word, which is what `pthread_join` waits on. `clone` without
`CLONE_THREAD` is the fork form, musl's `vfork` on aarch64 and riscv64
(`CLONE_VM | CLONE_VFORK` on the parent's stack: a fork, as the x86_64
`vfork` syscall is), or `posix_spawn`'s (`CLONE_VM | CLONE_VFORK` with a
stack, what Rust's `Command` uses through musl): a fork whose child starts
on that stack (the core's `fork_from`). Its memory is a fork's (the
parent's pages shared copy-on-write, `README.md`) rather than shared,
which posix_spawn and a `vfork` child that only execs do not notice:
posix_spawn's child reports a failed exec through a pipe the parent waits
on.

Signals: `rt_sigaction` (handlers with `SA_SIGINFO`, `SA_RESTART`,
`SA_NODEFER`, `SA_RESETHAND`, `sa_mask`), `rt_sigreturn`, `rt_sigprocmask`,
`rt_sigpending`, `rt_sigsuspend`, `rt_sigtimedwait`, `sigaltstack` (no
alternate stacks). Signal numbers and masks are translated between Linux and
the native (newlib) numbering, also in `wait4` statuses.

Sockets: `socket` (`AF_INET` stream and datagram, `AF_UNIX` stream),
`socketpair` (`AF_UNIX`), `connect`, `bind`, `listen`, `accept`, `accept4`
(`AF_UNIX`), `sendto`, `recvfrom`, `sendmsg`, `recvmsg` (with fds,
`SCM_RIGHTS`, on a unix socket: `docs/sockets-unix.md`), `shutdown`,
`getsockname`, `getpeername`, `getsockopt` (`SO_ERROR`, `SO_TYPE`,
`SO_SNDBUF`/`SO_RCVBUF`: 64 KiB for a unix socket, 8 KiB otherwise,
`SO_PASSCRED`; `SO_PEERCRED` gives the caller's own pid, uid and gid 0;
`SO_PEERSEC`, `SO_PEERGROUPS` and `SO_PEERPIDFD` are `ENOPROTOOPT`, as on
a kernel without them: zbus would take an answer of 0 for a pidfd and
close fd 0),
`setsockopt` (`SO_PASSCRED` on a unix socket: each read comes with the
sender's pid as `SCM_CREDENTIALS`, `/net/unix`'s `from`,
`docs/sockets-unix.md`; anything else accepted, ignored); `read`/`write`, `poll`, `fstat` and `ioctl(FIONBIO)` work on them
too. See Sockets below.

Time and misc: `clock_gettime` (the time of day for every clock but the
process's and the thread's CPU-time clocks, which are their CPU time),
`clock_getres`, `gettimeofday`, `time`, `getrusage` (the process's, the
thread's or the reaped children's CPU time, all as user time; the other
fields 0),
`nanosleep`, `clock_nanosleep`, `getrandom`; `memfd_create` (a file of `/dev/shm`
created and unlinked at once, as libgloss's; `F_ADD_SEALS` and
`F_GET_SEALS` keep and report a memfd's seals, from any fd naming it, and
do not enforce them; the last 256 sealed memfds are remembered); `poll` and `select` (x86_64), `ppoll` and `pselect6` (the same
readiness: readable on `POLLIN`, `POLLHUP` or `POLLERR`, writable on
`POLLOUT` or `POLLERR`, no exceptional conditions; the signal mask is not
applied and `select` does not update its timeout), which FLTK's event loop
waits in (dillo);
`sched_getaffinity` (every online CPU, so cargo and rustc run their jobs in
parallel) and `sched_setaffinity` (accepted; the core places tasks);
`fadvise64` (a hint: accepted); `membarrier` (the global and the private
expedited barriers, one and the same: every CPU running a task is
interrupted and answers, as for a TLB shootdown, before the call returns;
registering is accepted); the extended attribute calls (`listxattr`,
`getxattr`, `setxattr`, `removexattr` and their `l` and `f` forms): no
myos file system keeps attributes, so, as on a Linux file system without
them, a file lists none, reading one is `ENODATA` and setting or removing
one `EOPNOTSUPP` (what Python's `shutil.copy2`, so `ensurepip` and pipx,
copies and skips).

Anything else returns `ENOSYS`.
`/proc/linux_trace` reports each such call (Debugging below).

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
socket's `sendto`, `sendmsg`, `recvfrom` and `recvmsg` go through the
conversation's `hdata` (`docs/sockets-curl.md`): each datagram carries its
destination or its sender, so musl's resolver, which asks every
nameserver from one socket and takes the first answer, gets it from
whichever; `send`, `write`, `recv` and `read` are the bare datagrams of
`data`, to and from the peer `connect` set. A chrooted process reaches `/net` through the bind `linux --root` sets
up, and `get-alpine` gives a new root an `/etc/resolv.conf` for musl naming
the DNS servers the system uses: the `dns=` lines of `/net/ndb`, QEMU's
`10.0.2.3` without any. It is written once, when the root has none, so a
root moved to another network needs it edited or removed. (Under QEMU's
user network, a server listed after the lease's loses that one's answers:
QEMU keeps one host socket per guest source port, with the last address
sent to as its peer, and gives an answer the virtual nameserver's address
only while that peer is it; musl asks every server at once, from one
socket, and ignores an answer from an address it did not ask.) Every root also
gets Alpine's CA bundle (`ca-certificates-bundle`,
`/etc/ssl/certs/ca-certificates.crt`, where OpenSSL's `/etc/ssl/cert.pem`
points): Alpine has it from `alpine-base`, and `python3`, `curl` or `wget`
do not depend on it, so without it every TLS connection fails its
certificate check (`unable to get local issuer certificate`).

An `AF_UNIX` stream socket is a `/net/unix` conversation, mapped as
libgloss maps a native program's (`docs/sockets-unix.md`): `connect`
writes `connect NAME`, `listen` announces the name `bind` gave, `accept`
takes the next connection from the listener's `listen`, and an abstract
name (leading NUL) is `@name`. Names live in `/net/unix`, not in the
filesystem, so Linux and native programs reach each other by name, also
from inside a `linux --root`: an Alpine X client connects to the native
X server's `/tmp/.X11-unix/X0` with nothing in its root (libxcb tries the
abstract name first, is refused, and takes the path). Fds pass as
`SCM_RIGHTS` messages through the conversation's `rights` (Firefox's IPC
sends its sockets and shared memory this way); a received fd gets the
layer's state back from its path: a unix socket's conversation, a signal
fd's translation, a file's path.

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
   records it as `mmap` regions (so fork shares it and exit frees it);
4. starts the interpreter, with `AT_BASE` = its load address and
   `AT_PHDR` / `AT_ENTRY` naming the program.

The dynamic linker then loads the `DT_NEEDED` objects (and later `dlopen`
ones) from `/lib`, `/usr/local/lib`, `/usr/lib` with `open`, `read`,
`pread64` and `mmap` of the file, and maps each segment over the span it
reserved first. That needed core `mmap` work, which native programs share:

- file-backed mappings: `MAP_PRIVATE` (the pages are a private copy of
  the file, never written back) and `MAP_SHARED` (the pages are the file's,
  the page cache's frames for them, shared by every process that maps them
  and with `read` and `write`, and written back to the file by `msync`,
  `munmap`, exit and exec; through a read-only fd a shared mapping may not
  be written), and `MAP_SHARED | MAP_ANONYMOUS` (a tmpfs file no name
  reaches, shared with forked children: `fs::anon_file`; a tmpfs file is
  bytes in the kernel heap the mapping is written back to, so a segment
  costs its size twice, up to 16 MiB, issue #338);
- demand paging: a mapping takes no memory until it is used. The first
  touch of a page, a page fault from userspace or a kernel copy into a user
  buffer, gives it a frame, zeroed or read from the file
  (`user::fault_in`). A page of a private file mapping is the page cache's
  frame for it, shared by every process that maps it (`fs/pagecache.rs`)
  and mapped read-only, whatever the mapping's protection, until the first
  store to it: that store (a protection fault from userspace, or the
  kernel copy's check) gives the process its own copy of the page,
  writable (copy-on-write; `mprotect` to writable copies the same way). A
  fault on a file-backed page also maps the following pages of the
  region that the cache already holds, up to 16 in all (fault-around,
  read-only: a store takes the mapping's protection as above), so a
  program's code and data are paged in with a few faults rather than one
  per page from its second run on; `/proc/meminfo` counts the faults and
  the pages mapped around them. A fork shares every page of the parent's
  the same way, copy-on-write (`README.md`).
  rustc reserves 256 MiB for its allocator and maps
  some 250 MiB of libraries, and `rustc --version` touches about 30 MiB of
  it. A file changed or deleted while mapped gives its new contents (or
  zeros) to the pages not touched yet;
- `MAP_FIXED` replaces whatever is mapped in its range;
- `munmap` of any range in the `mmap` window (holes included) and
  `mprotect` of part of a mapping split the mapping;
- free address space is reused (first fit) instead of only growing;
- a larger window (64 GiB, on every arch: WebKit's JavaScript engine
  reserves 4 GiB aligned to 4 GiB) and 4096 mappings per process.

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
mkfs.ext2 /dev/nvme1n1/data && mount /dev/nvme1n1/data /mnt ext2
sh /lib/self-host.sh /mnt           # REV: the branch to build (master)
```

It installs Alpine's rust, cargo, rust-src, lld, clang, bash, busybox and
git into `DIR/alpine` with `get-alpine` (installed with `get-myos` first), clones (or updates) the source,
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
  posix_spawn's.
- No System V IPC (`shmget`, `semget`, `msgget`): a Linux program has
  POSIX shared memory (`shm_open`, `memfd_create`, `MAP_SHARED`).
- Close-on-exec (`O_CLOEXEC`, `FD_CLOEXEC`, `FIOCLEX`) and `O_NONBLOCK`
  outside sockets are flags the layer keeps per fd (`files.rs`): a
  successful exec closes the close-on-exec fds (the `on_exec` hook), and a
  non-blocking pipe end that would block gives `EAGAIN` (from the core's
  pipe readiness, as libgloss does for native programs).
- An eventfd is the kernel's event counter (`/dev/event/N/data`,
  `docs/events.md`), a timerfd its timer (`/dev/timer/N/data`) and an
  epoll fd its wait set (`/dev/wait/N/data`): the fd the program holds is
  the native one, read, written and polled by the core; `eventfd2`'s
  semaphore mode and initial value, `timerfd_settime` / `timerfd_gettime`
  and `epoll_ctl` are the object's `ctl` and `status`, `epoll_wait` waits
  for the set's readiness and takes the ready members (`KernelApi::
  wait_take`). A signalfd is a kernel signal fd (`/dev/sig/N/data`): the
  layer flags the fd and turns each 16-byte record a read gives into a
  `struct signalfd_siginfo` (`ssi_signo` Linux's number, the rest 0).
  `EPOLLET` reports a readiness once, and again only
  once something happened since (`docs/events.md`); `EPOLLPRI`,
  `EPOLLRDHUP`, `EPOLLEXCLUSIVE` and the signal mask of `epoll_pwait` are
  ignored. Every Linux
  clock is the one clock here, so a `TFD_TIMER_ABSTIME` value is taken
  relative to it, whatever the clock the timerfd was created on.
- `listen` and `accept` serve `AF_UNIX` sockets only: a TCP socket's
  `listen` is `EOPNOTSUPP` (issue #415).
- `AF_UNIX` sockets are `/net/unix` conversations (Sockets above), and
  `socketpair(AF_UNIX, SOCK_STREAM | SOCK_SEQPACKET)` a `/net/unix` pair,
  their ends kept like TCP sockets (Rust's `Command` reports a failed exec
  through one). Stream semantics for `SOCK_SEQPACKET` too: no message
  boundaries; no fd passing (`SCM_RIGHTS`: issue #340) and no
  `SOCK_DGRAM`.
- File locks are the native ones (`README.md`, "File locks"): `flock`,
  `fcntl`'s record locks (`F_SETLK`, `F_SETLKW`, `F_GETLK`) and the open
  file description's (`F_OFD_SETLK`, ...), so cargo, SQLite and git
  guard against each other as on Linux. `ftruncate` and `O_CREAT|O_EXCL`
  are native too (any length; the name checked and created in one step).
  `O_NOFOLLOW` and `O_DIRECTORY` are kept (`ELOOP`, `ENOTDIR`). `link` and
  `linkat` fail with `EPERM`: myos has no hard links.
  `pwrite` past the end of a file leaves zeros in the gap.
- `chmod`, `chown` and `fsync` succeed and do nothing (`lchown` and the
  `at` forms with `AT_SYMLINK_NOFOLLOW` on a dangling link too): what a program may
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
- Sockets: IPv4 clients and `AF_UNIX` streams only (no `AF_INET`
  `listen`/`accept`, no IPv6); no half-close (`shutdown` hangs up only for
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
  - a per-process `mmap` window of 64 GiB (every arch: the process span
    is 65 GiB, the image, stack and heap in its first gigabyte) with at
    most 4096 mappings; adjacent mappings with the same
    protection and backing are merged (musl's malloc makes hundreds of
    small neighbouring ones: jq peaks at 188; rustc's Scudo allocator
    needs more than 256 to compile `core`). At most 1024 distinct files
    are mapped at once (Firefox and its GTK stack map well over a
    hundred); a file mapping past that is read in whole when it is made;
  - a 16 MiB `brk` heap;
  - 256 fds per process, 2048 open file descriptions in the system, 512
    tasks (threads and processes) in total, 64 pipes in the system (and 24 named FIFOs), each with a
    4 KiB buffer;
  - `/tmp` (tmpfs) files of at most 16 MiB each, all of them in the
    kernel heap (a quarter of the memory, 64 MiB to 1 GiB and at most
    half its largest region: 1 GiB in the 4 GiB CI guests; the boot's
    `[ OK ] heap: N MiB` line says how much).

## Debugging

A program that exits without a word usually asked for something the layer
does not have, or crashed. These say which:

- **The trace.** `echo 1 > /proc/linux_trace` (root: writing `/proc` is
  the policy's `proc {all}`, `docs/security.md`) makes the layer report
  every syscall it does not implement on the console as it answers
  `ENOSYS`, with the process and the first three arguments, in the
  arch's own numbering (x86_64's, or the generic table of aarch64 and
  riscv64):
  ```
  linux: pid 7 syscall 4000 (0x1, 0x2, 0x3): ENOSYS
  ```
  `echo 0 > /proc/linux_trace` turns it off; it starts off.
- **What a hung or spinning program is doing.** `echo 2 >
  /proc/linux_trace` also keeps every thread's last 8 syscalls with
  their answers and its count of calls, without printing them. Each
  write publishes what was kept so far, after the level, so with the
  program running in the background, write `2` again and read the file:
  ```
  2
  tid 9 pid 9: 1520 calls
    202 (0x7f1c0, 0x80, 0x0, 0x0) ...
  tid 11 pid 9: 4800213 calls
    24 (0x0, 0x0, 0x0, 0x0) = 0x0
  ```
  The thread whose count climbs between two reads is the busy one; a
  call with `...` is still waiting (`202` is x86_64's `futex`, `24` its
  `sched_yield`). A thread spinning without syscalls keeps its count
  still: its last calls show what it last asked for. Each write keeps its copy of the
  text (procfs files do not give theirs back), under 1 KiB per thread.
- **Where a fault was.** The kernel's `user fault` line names the file
  and the ELF virtual address of the faulting instruction when it is in
  a file mapping, which is where a shared library's code is:
  ```
  [ WARN ] user fault: page fault in /lib/libsmoke.so+0x15a0 cr2=0x0 rip=0x80011bb5a0 ...
  ```
  The address is what `nm`, `objdump --start-address` and `addr2line -e
  FILE` take, on a copy of the same file on the host. The dynamic linker
  (musl's libc too) and the last page of each segment of a program are
  copied at exec and say `in anonymous memory` (issue #418). A write to
  address 0 (`cr2=0x0`, `code=0x6` on x86_64) from a program's own code
  is often deliberate: Firefox's `MOZ_CRASH` and `MOZ_RELEASE_ASSERT`
  store a reason and write to 0, and the reason is the string the code
  loads just before the write.

## Testing

The boot tests (`docs/testing.md`, `user/tests/kernel.sh`) run
`linux /bin/linux/linux-smoke` and `linux /bin/linux/linux-dyn` on every
arch when the image was built with `--features linux_compat`, and expect
`LINUX-SMOKE OK` (files, directories, mmap, fork/execve/wait4, pipes, Linux
signal numbers, handlers, masks, `sigwait`, `EINTR` and `SA_RESTART`,
pthreads: a mutex, a condition variable, thread-local storage, thread ids,
join, and `exit` from a thread ending the process) and `LINUX-DYN OK` (a
call, shared data, a relocated function pointer and a thread-local in
`libsmoke.so`, `printf` from `libc.so`, `dlopen`/`dlsym` of `libsmoke2.so`);
`linux --root` of a bare root with the smoke's event fd checks
(`linux-smoke evfd`, `LINUX-EVFD OK`) runs them through the root's bind
of `/dev`, and with a `resolv.conf` naming the system's nameserver and
then a dead address, musl's resolver (`linux-smoke dns`, `LINUX-DNS OK`)
gets the answer from the live one; `linux_trace` turns the trace on, checks that a syscall no arch
has still gets `ENOSYS` (`linux-smoke enosys`), and off again. A fault
cannot be a boot test (the host takes the kernel's `user fault` line for a
crash), so the fault's location is not tested in the boot.
The test is plain musl C, so it can also be run on a Linux host for
reference. The full list then runs
`run-myos get-alpine jq && linux --root /data/alpine jq -Rrn '...' /proc/mounts` (the
default root on the test disk's data partition, bootstrapped by get-alpine
and installed by apk under the layer, recorded in `/data/alpine/lib/apk/db`), which counts the binds of `/dev`, `/proc`
and `/net` it sees in the root and expects `ALPINE-JQ 6`, then installs
Python into `/tmp/alpine` (python3 and its 19 dependencies, ~45 MB, more
than the 64 MiB data partition holds) and runs
`python3 -c 'import json,sqlite3;print("PYTHON",json.loads("[42]")[0])'`
(the standard library and two C extension modules), expecting `PYTHON 42`,
and fetches `http://example.com/` with `urllib` (DNS over UDP, then TCP),
expecting `HTTP 200`, and runs an asyncio script (`alpine_python_asyncio`:
the end-to-end test of the event fds, `docs/events.md`), whose selector is
`select.epoll` over `/dev/wait`, whose signal handler is a wakeup fd in it
and whose subprocess is watched from a thread: an echo over an `AF_UNIX`
socket, a timer, a timeout, `SIGUSR1` handled by the loop and a child's
output,
expecting `ASYNCIO EpollSelector ASYNCIO True True USR1 sub 0`. Last, it
mounts the disk the launcher made on the
host with `linux-compat/alpine-disk.sh ARCH target/alpine-rust-ARCH.img
rust` (kept in `target/`: remove it for newer packages) and attached as
`/dev/nvme2n1/data` (writes go to a QEMU snapshot), and runs `rustc --version`
from it, expecting `rustc 1.`. They need the Alpine mirror and
`example.com` to be reachable.

Without the feature (the quick list, the normal PR CI), the test instead
expects the module in `/proc/modules`, loaded by the boot script's
default `rc.local` with `insmod`, and a second `insmod` refused: the
module is built and loadable in every build.

CI builds the musl pieces when their inputs change (`linux-compat` in
`ci-ports.yml`, cached in the GHCR registry by `scripts/ci-registry.sh`
under a hash of `linux-compat/` and newlib) and packs them into
`ci-build.tar.zst`. The full boot (daily, or `full_boot` on dispatch;
`MYOS_CI_FEATURES=linux_compat`) builds all four disk images with the
layer, so bios, uefi, aarch64 and riscv64 run the tests above. On master
the **iso** job builds the x86_64 hybrid ISO with `--features linux_compat`,
so the downloadable ISO always carries the layer (module loaded at boot,
`linux-smoke`, `linux-dyn`; `get-alpine` comes from the release's packages).
There are no separate Linux jobs.

`linux-compat/build.sh` builds musl (static and shared) with clang for each
target (and skips itself when its outputs match its stamp,
`target/.myos-linux-compat-version`). On aarch64/riscv64 musl's `long double` is 128-bit and needs
compiler-rt's quad-float builtins: the script fetches those sources (pinned
LLVM tag) and links them into `libc.so`. The static smoke test is not linked
against them, so it (and the launcher) avoid `printf`.
