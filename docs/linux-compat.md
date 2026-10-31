# Linux syscall compatibility layer (optional)

myos can run some unmodified Linux binaries: programs linked against musl,
for x86_64, aarch64 or riscv64, static-PIE or dynamically linked. The
layer is an optional add-on. myos's own syscall ABI stays the primary
interface, and nothing about native myos programs changes when the layer
is loaded.

The layer is a kernel module, `linux` (`modules/linux`). The kernel itself
has no Linux code, only a generic *personality* hook
(`kernel/src/personality.rs`) that the module fills through the module
ABI.

- Every image carries the module, under `/lib/modules/linux`, and the
  `linux` launcher.
- The `linux_compat` Cargo feature (off by default) loads the module at
  boot and adds the musl test binaries.
- `get-alpine` installs Alpine Linux packages, with Alpine's own `apk` run
  under the layer. It is a myos package (`get-myos get-alpine`).

## Running Linux programs

A Linux program runs when it is started through the `linux` launcher:

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

In a default build the kernel does not load the module at boot. The boot
script's default `/data/etc/rc.local` loads it (`docs/install.md`) with
`insmod /lib/modules/linux`. The console then says `[ OK ] linux`, and the
module is listed in `/proc/modules`. Nothing of the layer but the module
and the launcher is needed to run Alpine's packages.

## Turning it on

| Where | What |
|-------|------|
| `kernel/build.rs` | builds `modules/linux` for the 3 arches like every module |
| `src/limine_image.rs` | `OPTIONAL_MODULES`: `linux` is in `/lib/modules/boot.list` only with `linux_compat`, in `/lib/modules` always |
| `Cargo.toml` (root) | feature `linux_compat` (not in `default` or `core`) |
| `linux-compat/build-launcher.sh` | the `linux` launcher for each arch, in every image |
| `linux-compat/build.sh` | musl and the Linux test binaries for each arch (run by `build.rs` when the feature is on) |

Without the module loaded no personality is registered, so `linux
PROGRAM` prints an error. The test programs under `/bin/linux/` belong to
the tests module (`docs/testing.md`): they are on the build's disks only,
never in the ISO.

## Alpine Linux packages: `get-alpine`

The image carries nothing from Alpine. `get-alpine`
(`packages/get-alpine/get-alpine.c`, a native program and a package,
`docs/packages.md`) bootstraps a Linux root at run time, and then has
Alpine's own package manager, `apk`, install the packages into it. The
layer must be loaded, because apk runs under it.

```sh
run-myos get-alpine [-r ROOT] [-u] [-n] [PACKAGE...]   # ROOT: /data/alpine or /tmp/alpine
linux --root ROOT apk add|del|upgrade|info ...      # apk itself, afterwards (no PACKAGE: just the bootstrap)
linux --root ROOT PROGRAM [ARG...]
```

It works in two steps:

1. **Bootstrap**, once per root, natively with the guest's `curl`. It
   reduces Alpine's `main` and `community` indexes to
   `ROOT/.get-alpine/index`, then fetches, checks and unpacks what apk
   needs to run: `apk-tools`, `alpine-keys`, `busybox`, `alpine-baselayout`
   and `ca-certificates-bundle` (the TLS trust store, which python3, curl
   or wget alone do not pull in), with their dependencies. Each package's
   checksums are checked against the index before anything is written.
   The index signature is not checked: the download is HTTPS, and apk
   checks it next with the keys just unpacked. It also writes the root's
   `/etc/apk/repositories` and an `/etc/resolv.conf` (see Sockets).
2. **apk.** It runs `linux --root ROOT apk add --initdb BASE...
   PACKAGE...`, so apk's database owns the bootstrap set too and the
   install scripts and triggers run. From then on `linux --root ROOT apk
   ...` is the whole package manager. `-u` refreshes the indexes first.

`-n` leaves apk out: the packages are unpacked natively like the
bootstrap set, their install scripts do not run, and the layer need not
be loaded. `linux-compat/alpine-disk.sh` uses it on the host.

### Running a program in a root

`linux --root ROOT` chroots into `ROOT` before the exec, with a Linux
`PATH`, so the program finds its dynamic linker
(`/lib/ld-musl-<arch>.so.1`), libraries and data at their Alpine paths.
Before the chroot it bind-mounts the system's `/dev`, `/proc` and `/net`
into `ROOT`. Binds last until reboot or `umount TARGET`.

### Mirror, branch and root

- `ALPINE_MIRROR` overrides `https://dl-cdn.alpinelinux.org/alpine`.
- `ALPINE_BRANCH` overrides `latest-stable`.
- The default root is `/data/alpine` when the data partition is mounted
  at `/data` (`docs/install.md`), kept across reboots and upgrades.
- Otherwise it is `/tmp/alpine`, a tmpfs in the kernel heap: room for a
  few small packages, gone at reboot.

`etc/policy` labels `/data/alpine` `sys.pkg`, as it labels `get-myos`'s
apps: a user's shell runs what is there, and the administrator installs
it.

### Big roots

A root too big for the data partition goes on another disk. For
instance, Alpine's Rust compiler (~300 MB of downloads, ~600 MB
installed) on the scratch disk of a test boot:

```sh
mkfs.ext2 /dev/nvme1n1/data && mount /dev/nvme1n1/data /mnt ext2
run-myos get-alpine -r /mnt/alpine rust
linux --root /mnt/alpine rustc --version
```

Under emulation that takes a couple of hours. The mirror drops long
transfers; a package apk lost is installed by the next run. Making the
root on the host is quicker:

```sh
linux-compat/alpine-disk.sh ARCH OUT.img [NAME=]PACKAGE[,...]...
```

It builds get-alpine for the host and unpacks the packages for ARCH with
`-n` (no install scripts, empty apk database). `NAME=` makes a root
`/NAME`, so one image can hold several. The result is an ext2 image to
attach as a disk.

## Building myos in myos

For myos programs (the `*-unknown-myos` targets and their std), the `rust`
package sets the pinned nightly's own toolchain up in an Alpine root and
runs cargo there, on every arch (`packages/rust/README.md`).

For the kernel, `linux-compat/self-host.sh DIR [REV]` (in the image as
`/lib/self-host.sh`) builds the x86_64 kernel inside myos with Alpine's
Rust toolchain:

```sh
mkfs.ext2 /dev/nvme1n1/data && mount /dev/nvme1n1/data /mnt ext2
sh /lib/self-host.sh /mnt           # REV: the branch to build (master)
```

It installs Alpine's Rust toolchain, clang and git into `DIR/alpine`,
clones the source, builds `core` and `alloc` for `x86_64-unknown-none`
(Alpine ships them for its own target only), and runs `cargo build -p
kernel` under `linux --root`. Each step is skipped when its result is
there. The scratch disk needs a few GiB. The kernel it builds boots and
passes `test-mini`. Under TCG a first run takes hours.

Known gaps: the C programs the kernel embeds (the std demos, c-hello,
oksh, getty, login) and `http` (LibreSSL over newlib: autotools, `make`,
`python3`) are not built in myos. They are copied from the running
system; `MYOS_PREBUILT=http` has `kernel/build.rs` take `http` as it is.

## How it works

### The Linux personality

Linux musl binaries and myos newlib binaries are both plain SYSV ELFs, so
the kernel does not guess. A process has the **Linux personality** only
if:

1. `linux PROGRAM ...` (`linux-compat/launcher.c`) calls
   `SYS_LINUX_NEXT_EXEC` (51) and then execs `PROGRAM`; or
2. a process that already has it calls Linux `execve`.

`fork` inherits the personality, and a native exec clears it. A native
myos binary does not work under `linux`: its syscalls get decoded as
Linux ones, and it dies of its own fault.

A *personality* is a foreign syscall ABI that a module registers
(`KernelApi::personality_register`, `docs/modules.md`). The kernel keeps
an *active* and a *pending* bit per task slot and calls the module's
`PersonalityOps` at a few generic points: an active task's syscalls,
spawn/fork/thread/exec (so the module can keep per-task state), and the
delivery of a caught signal (the module writes the foreign frame). On
exec the kernel also gives every image the SysV auxv ids, without which
musl acts as setuid and ignores `LD_LIBRARY_PATH`. Everything else the
module needs is plain `KernelApi` services (`modules/abi/src/lib.rs`).

### Pieces

| Where | What |
|-------|------|
| `kernel/src/personality.rs` | the generic side: the per-slot bits, the registered ops, the hooks |
| `modules/linux/src/main.rs` | `module_init`, registering the ops |
| `modules/linux/src/k.rs` | wrappers over `KernelApi` |
| `modules/linux/src/{x86_64,aarch64,riscv64,generic}.rs` | per-arch syscall numbers, `struct stat`, signal frames |
| `modules/linux/src/sys.rs` | the syscall handlers: decode, call the native implementation, return `-errno` |
| `modules/linux/src/signal.rs`, `thread.rs`, `net.rs`, `files.rs`, `abi.rs` | signals, `clone`/`futex`, sockets, per-fd state, errno and signal translation |
| `modules/linux/src/trace.rs` | `/proc/linux_trace` |
| `linux-compat/launcher.c` | the `linux` command |
| `linux-compat/tests/` | the musl smoke tests, static and dynamic |
| `packages/get-alpine/get-alpine.c` | the Alpine package installer |

### Dynamic linking

For a Linux exec of a program with a `PT_INTERP` (musl's
`/lib/ld-musl-<arch>.so.1`, which is `libc.so` itself), the kernel reads
the interpreter first, so a missing one fails the exec cleanly. It maps
the program and the interpreter without relocating them, and starts the
interpreter with `AT_BASE` set (`exec_interp` in
`kernel/src/user/syscall.rs`). The dynamic linker then maps the shared
objects from `/lib`, `/usr/local/lib` and `/usr/lib` with `mmap`.

That relies on the core's `mmap`, which native programs share:

- private and shared file mappings, through the page cache
  (`docs/fs.md`, "Page cache");
- demand paging: a page takes a frame at its first touch, and a private
  file page stays the cache's shared frame until a store copies it;
- fault-around: a fault on a file page also maps up to 16 pages of the
  region the cache already holds (`FaultAround` in `/proc/meminfo`), so a
  program's second run pages in with a few faults;
- `MAP_FIXED`, partial `munmap` and `mprotect`, and reuse of freed
  address space;
- a 64 GiB window per process (WebKit's JavaScript engine reserves 4 GiB
  aligned to 4 GiB), with 4096 mappings.

For scale, rustc reserves 256 MiB and maps some 250 MiB of libraries, and
`rustc --version` touches about 30 MiB of it. POSIX shared memory
(`shm_open`, `memfd_create`, `MAP_SHARED | MAP_ANONYMOUS`) is the native
one (`docs/fs.md`, "Shared memory").

### Signal handlers

Delivery reuses the native decision path: masks, `SA_RESTART`, `EINTR`,
`sigsuspend` (`docs/signals.md`). Only the frame differs: the module
writes a Linux `rt_sigframe` with the registers, the mask and the FP/SIMD
state, and the handler returns through `rt_sigreturn`. musl passes
`SA_RESTORER` on x86_64 and aarch64; riscv64 has none (Linux uses its
vDSO), so the module maps a one-page trampoline. A fault's signal carries
`si_code`, `si_addr` and the fault address, and `rt_sigreturn` resumes the
faulting instruction. On x86_64 the callee-saved registers read 0 in
`uc_mcontext` and are not restored from it.

### Sockets

A Linux socket is a conversation of the native `/net` (`docs/net.md`,
`modules/linux/src/net.rs`). Its fd is the conversation's `data` file, so
reads, writes, `dup`, `fork` and `close` are ordinary fd operations.

- `socket` opens `/net/{tcp,udp}/clone`; `connect` writes `connect
  a.b.c.d!port` to `ctl`. An IPv4-mapped `AF_INET6` peer is named back as
  `::ffff:a.b.c.d`, as Linux does.
- The `/net` files never block. A blocking read or connect sleeps until
  netd's next reply; a write retries while netd's ring or the TCP send
  room (8 KiB) is full.
- A datagram socket's `sendto`/`recvfrom`/`sendmsg`/`recvmsg` go through
  the conversation's `hdata`, which carries each datagram's address. So
  musl's resolver, which asks every nameserver from one socket, takes the
  answer from whichever server sends it.
- An `AF_UNIX` stream socket is a `/net/unix` conversation, mapped as
  libgloss maps a native program's (`docs/sockets-unix.md`). Names live
  in `/net/unix`, not the filesystem, so Linux and native programs reach
  each other by name, also from inside a `linux --root`: an Alpine X
  client reaches the native X server with nothing in its root. Fds pass as
  `SCM_RIGHTS` through the conversation's `rights`.

`get-alpine` gives a new root an `/etc/resolv.conf` naming the system's
DNS servers (the `dns=` lines of `/net/ndb`, or QEMU's `10.0.2.3`). It is
written only when the root has none, so a root moved to another network
needs it edited. Under QEMU's user network a server listed after the
lease's loses that one's answers, because QEMU rewrites the answer's
source only while the socket's last peer is its nameserver.

Every root gets Alpine's CA bundle (`/etc/ssl/certs/ca-certificates.crt`).
Without it every TLS connection fails with `unable to get local issuer
certificate`.

## Supported syscalls

x86_64 has its own syscall table; aarch64 and riscv64 share the
`asm-generic` one, which has only the `*at`, `clone`, `ppoll` and `dup3`
forms of the legacy calls. Anything not handled returns `ENOSYS`, and
`/proc/linux_trace` reports it (see Debugging). The handlers are in
`modules/linux/src/sys.rs`; the groups, with what differs from Linux:

- **Files**: the usual file, directory, fd and `poll` calls. `ioctl`
  covers the tty requests (`docs/tty.md`) and `FIONBIO`. `getdents64`
  reads a snapshot of the directory. `fallocate` takes mode 0 only. The
  xattr calls behave as on a filesystem without attributes (`ENODATA`,
  `EOPNOTSUPP`). `statfs` has no filesystem magic.
- **Memory**: `brk`, `mmap`, `munmap`, `mprotect`, `msync`;
  `madvise(MADV_DONTNEED)` drops pages, other advice is ignored. `mremap`
  only shrinks (issue #421: musl's `realloc` copies instead).
- **Processes**: `fork`, `vfork` (as fork), `execve` (`#!` scripts run
  the kernel's way, `docs/security.md`), `wait4`, `waitid` (exits only),
  the pid, session and group calls, `uname` (the node name is
  `/proc/sys/kernel/hostname`), `sysinfo`, `reboot` (`docs/power.md`),
  `prlimit64`. uid and gid are always 0: what a program may do is the
  security policy's (`docs/security.md`). `prctl` accepts a few options;
  `PR_SET_PDEATHSIG` is per process, not per parent thread. No
  namespaces (`unshare`, `CLONE_NEW*` are `EINVAL`), no capabilities, no
  seccomp. `/proc/sys/kernel/overflowuid` and `overflowgid` read 65534.
- **Threads**: `clone` with `CLONE_THREAD` makes a native thread
  (`docs/threads.md`). `futex` maps onto the core's `wait_addr` /
  `wake_addr`; a bitset is "all" and a requeue wakes every waiter.
  `clone` without `CLONE_THREAD` is a fork, including musl's `vfork` and
  `posix_spawn`'s `CLONE_VM | CLONE_VFORK` with a stack (the child starts
  on that stack).
- **Signals**: `rt_sigaction` (`SA_SIGINFO`, `SA_RESTART`, `SA_NODEFER`,
  `SA_RESETHAND`), `rt_sigreturn`, the mask, pending, suspend and
  timed-wait calls. Signal numbers are translated, also in `wait4`
  statuses. `sigaltstack` offers no stacks.
- **Sockets**: see Sockets above and Limits. `getsockopt` answers
  `SO_ERROR`, `SO_TYPE`, the buffer sizes (64 KiB unix, 8 KiB otherwise)
  and `SO_PEERCRED` (the caller's pid, ids 0); `SO_PEERSEC`,
  `SO_PEERGROUPS` and `SO_PEERPIDFD` are `ENOPROTOOPT`. `setsockopt`
  honours `SO_PASSCRED` on a unix socket and ignores the rest.
- **Time**: `clock_gettime` with the realtime, monotonic, boottime and
  CPU-time clocks; `gettimeofday`, `nanosleep`, `clock_nanosleep`
  (absolute times on their clock), `getrusage` (CPU time as user time).
- **Event fds**: `eventfd`, `timerfd_*`, `epoll_*` and `signalfd` (see
  Limits).
- **Other**: `getrandom`; `memfd_create` (a `/dev/shm` file; seals are
  kept for the last 256 sealed memfds but not enforced); `select`,
  `pselect6` and `ppoll` (one readiness; the signal mask is not applied);
  `sched_getaffinity` (every online CPU); `membarrier` (every CPU running
  a task answers an IPI); `fadvise64` and `readahead` as hints.

## Limits

- **Binaries.** musl binaries only, static or dynamic, PIE or not. glibc
  ones are not tried and need more syscalls.
  - A position-independent program loads at 1 GiB.
  - A non-PIE (`ET_EXEC`) program loads at its link address (Alpine's gcc
    links at 0x400000 on x86_64, 0x10000 or 0x400000 elsewhere). It must
    leave the first 64 KiB unmapped and room above it in the user
    window: `[0, 512 GiB)` on x86_64, `[0, 66 GiB)` on aarch64 and
    riscv64.
- **Signals** act at syscall exit, as for native programs: a task looping
  in user mode is interrupted only at its next syscall, except by
  `SIGKILL`. A fault's signal acts at once. No alternate signal stacks,
  no real-time signal queueing.
- **`clone`.** The only `clone` with `CLONE_VM` but not `CLONE_THREAD` is
  posix_spawn's. Its child's memory is a fork's (copy-on-write), not
  shared; posix_spawn and a `vfork` child that only execs do not notice.
- **No System V IPC** from Linux programs (`shmget`, `semget`, `msgget`).
  POSIX shared memory works.
- **Close-on-exec and `O_NONBLOCK`** (outside sockets) are flags the layer
  keeps per fd (`files.rs`). Exec closes the close-on-exec fds, and a
  non-blocking pipe gives `EAGAIN`.
- **Event fds** are the kernel's objects (`docs/events.md`): an eventfd
  is `/dev/event/N`, a timerfd `/dev/timer/N`, an epoll fd `/dev/wait/N`,
  a signalfd `/dev/sig/N` (the layer turns its records into `struct
  signalfd_siginfo`, with only `ssi_signo` set). `EPOLLET` reports a
  readiness once until the object changes. `EPOLLPRI`, `EPOLLRDHUP`,
  `EPOLLEXCLUSIVE` and `epoll_pwait`'s mask are ignored.
- **Sockets.** IPv4 and IPv6 clients and `AF_UNIX` streams only.
  - `listen` and `accept` serve `AF_UNIX` only; TCP's `listen` is
    `EOPNOTSUPP` (issue #415).
  - `socketpair` makes `AF_UNIX` `SOCK_STREAM` or `SOCK_SEQPACKET` pairs.
    A `SOCK_SEQPACKET` record is at most 8 KiB (`EMSGSIZE`). No
    `SOCK_DGRAM`.
  - No half-close: `shutdown` hangs up only for `SHUT_RDWR`.
  - A TCP socket's local address reads as the any address, port 0; a UDP
    one's is netd's.
  - Options are ignored, `IPV6_V6ONLY` too.
  - `poll` reports a connected socket writable even with no send room;
    a nonblocking write then says `EAGAIN`.
- **File locks** are the native ones (`flock`, `fcntl` record and OFD
  locks), so cargo, SQLite and git guard against each other as on Linux.
- **Files.**
  - `link` and `linkat` fail with `EPERM`: myos has no hard links.
  - `chmod`, `chown` and `fsync` succeed and do nothing. Access is the
    security policy's, and `stat`'s permission bits are the owner's only,
    saying what the caller may do.
  - `stat` reports times in whole seconds on tmpfs and ext2, with
    `st_ctime` the modification time; other filesystems report 0.
    `utimensat`, `futimens` and `utimes` work there and fail with `EROFS`
    elsewhere.
- **The native limits apply:**
  - exec: up to 1024 arguments and 1024 environment strings, at most
    128 KiB together.
  - A dynamically linked PIE program (most of Alpine's) is mapped from its
    file like a shared object, so only the mmap window bounds its size.
  - Any other program is read whole: a file of at most 16 MiB from a
    writable filesystem (the initramfs has no limit), whose image spans
    at most 4096 pages (16 MiB). A native PIE program past either limit
    is mapped from its file instead and relocated in place; if that
    fails, it exits with status 127 and a `[ FAIL ] exec PATH: ...`
    console line.
  - A 64 GiB `mmap` window per process with at most 4096 mappings
    (adjacent ones merge; rustc's allocator needs more than 256 to
    compile `core`).
  - At most 1024 distinct files mapped at once; a file mapping past that
    is read in whole.
  - A 256 MiB `brk` heap.
  - 256 fds per process, 2048 open file descriptions in the system, 512
    tasks in total, 64 pipes (and 24 named FIFOs) of 4 KiB each.
  - `/tmp` files of at most 128 MiB each, all in the kernel heap (a
    quarter of the memory, 64 MiB to 1 GiB; the boot's `[ OK ] heap: N
    MiB` line says how much). `/data` is 64 MiB on a boot disk written
    from the image (`docs/install.md`).

## Debugging

A program that exits without a word usually asked for something the
layer does not have, or crashed.

**The trace.** `echo 1 > /proc/linux_trace` reports on the console every
syscall the layer does not implement, with the process and the first
three arguments, in the arch's own numbering. `echo 0` turns it off; it
starts off. Writing `/proc` needs root (`docs/security.md`).

```
linux: pid 7 syscall 4000 (0x1, 0x2, 0x3): ENOSYS
```

**What a hung or spinning program is doing.** `echo 2 >
/proc/linux_trace` also keeps every thread's last 8 syscalls and its call
count. Each write publishes what was kept so far, so with the program
running in the background, write `2` again and read the file:

```
2
tid 9 pid 9: 1520 calls
  202 (0x7f1c0, 0x80, 0x0, 0x0) ...
tid 11 pid 9: 4800213 calls
  24 (0x0, 0x0, 0x0, 0x0) = 0x0
```

The thread whose count climbs between two reads is the busy one. A call
with `...` is still waiting (`202` is x86_64's `futex`, `24` its
`sched_yield`). A thread spinning without syscalls keeps its count still.

**Where a fault was.** The kernel's `user fault` line names the file and
the ELF address of the faulting instruction when it is in a file mapping,
which is where a shared library's code is:

```
[ WARN ] user fault: page fault in /lib/libsmoke.so+0x15a0 cr2=0x0 rip=0x80011bb5a0 ...
```

- `nm`, `objdump --start-address` and `addr2line -e FILE` take that
  address, on a copy of the same file on the host.
- The dynamic linker (musl's libc too) and the last page of each program
  segment are copied at exec, and say `in anonymous memory` (issue #418).
- A write to address 0 from a program's own code is often deliberate:
  Firefox's `MOZ_CRASH` stores a reason and writes to 0.

## Testing

The boot tests are in `user/tests/kernel.sh` (`docs/testing.md`), tagged
`linux`; the test programs are plain musl C (`linux-compat/tests/`), so
they also run on a Linux host for reference.

- **With the feature**, every arch runs the static and dynamic smokes
  (`LINUX-SMOKE OK`, `LINUX-DYN OK`), the non-PIE builds, event fds and
  DNS in a bare root, and the trace. The full list then installs `jq`
  with `get-alpine` and apk, and runs Python (with `urllib` and asyncio
  over epoll), an X client and `rustc --version` from the Alpine disk.
  A fault a program does not catch cannot be a boot test (the host takes
  the `user fault` line for a crash), so the fault location is not
  tested.
- **Without the feature**, the test expects the module loaded by
  `rc.local` and a second `insmod` refused.

The Alpine disk is `target/alpine-ARCH.img`, made on the host by
`linux-compat/alpine-disks.sh ARCH` (three roots: Python, X11, Rust, with
`get-alpine -n`). It is attached as `/dev/nvme2n1/data`, with writes to a
QEMU snapshot; `docs/testing.md` says how CI caches it.

In CI the musl pieces are built and cached like a port (`linux-compat` in
`ci-ports.yml`). A boot whose selection has a `linux` test builds its
images with the layer (`MYOS_CI_FEATURES=linux_compat`; `docs/testing.md`,
"In CI"), and the full boot does so on every arch. On master the **iso**
job builds the x86_64 ISO with `--features linux_compat`, so the
downloadable ISO loads the layer at boot.

### Building the musl pieces

`linux-compat/build.sh` builds musl (static and shared) with clang for
each target, and skips itself when its stamp
(`target/.myos-linux-compat-version`) matches.

- riscv64 binaries are built for Alpine's ISA, `rv64gc`, named explicitly
  because clang's default follows the host distribution (Ubuntu 26.04's
  emits RVA23 vector instructions QEMU's default CPU lacks).
- On aarch64 and riscv64 musl's 128-bit `long double` needs compiler-rt's
  quad-float builtins, fetched from a pinned LLVM tag and linked into
  `libc.so`. The static smoke and the launcher are not linked against
  them, so they avoid `printf`.
