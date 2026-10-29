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
module is listed in `/proc/modules`.

From then on, `linux PROGRAM` works for any Linux musl binary at hand, and
the lines above from `get-myos get-alpine` on work the same. Nothing of
the layer but the module and the launcher is needed to run Alpine's
packages.

## Turning it on

| Where | What |
|-------|------|
| `kernel/build.rs` | builds `modules/linux` for the 3 arches like every module (`target/linux-<triple>`) |
| `src/limine_image.rs` | `OPTIONAL_MODULES`: `linux` is in `/lib/modules/boot.list` only with `linux_compat`, in `/lib/modules` always |
| `Cargo.toml` (root) | feature `linux_compat` (not in `default` or `core`) |
| `linux-compat/build-launcher.sh` | the `linux` launcher for each arch, in every image (`build.rs` runs it) |
| `linux-compat/build.sh` | musl and the Linux test binaries for each arch (run by `build.rs` when the feature is on) |

Without the module loaded, the kernel has no personality registered.
`SYS_LINUX_NEXT_EXEC` (51) fails, so `linux PROGRAM` prints an error, and
no task can get the Linux personality.

The test programs under `/bin/linux/`, with the `libsmoke` objects and
musl's loader they use, belong to the tests module (`docs/testing.md`).
They are built with the feature and are on the build's disks only, never
in the ISO.

## Alpine Linux packages: `get-alpine`

The image carries nothing from Alpine. `get-alpine`
(`packages/get-alpine/get-alpine.c`) bootstraps a Linux root at run time,
and then has Alpine's own package manager, `apk`, install the packages
into it.

- It is a native program for x86_64, aarch64 and riscv64. Alpine has
  repositories for all three.
- It is a package (`docs/packages.md`), not in the image. `get-myos
  get-alpine` installs it as an app, and `run-myos get-alpine` runs it.
- The layer must be loaded, because apk runs under it.

```sh
run-myos get-alpine [-r ROOT] [-u] [-n] [PACKAGE...]   # ROOT: /data/alpine or /tmp/alpine
linux --root ROOT apk add|del|upgrade|info ...      # apk itself, afterwards (no PACKAGE: just the bootstrap)
linux --root ROOT PROGRAM [ARG...]
```

### What `get-alpine` does

1. **Bootstrap.** This happens once per root, natively, with the guest's
   `curl`.
   - It downloads the `main` and `community` indexes (`APKINDEX.tar.gz`)
     and reduces them, streaming, to `ROOT/.get-alpine/index`. That file
     has one line per package, with its repository, version, control
     checksum, dependencies and provides (`so:`, `cmd:`, ...).
   - Through that index it fetches what apk needs to run, with their
     dependencies (`musl`, `libapk`, `libcrypto3`, ...):
     - `apk-tools`;
     - `alpine-keys`, the keys apk checks the repositories with;
     - `busybox` and `busybox-binsh`: `/bin/sh` and the tools install
       scripts use;
     - `alpine-baselayout`: the users and directories they expect;
     - `ca-certificates-bundle`: the trust store of every TLS client,
       which python3, curl or wget alone do not pull in.
   - It checks each package before writing anything. An `.apk` is three
     concatenated gzip members: signature, control, data. The SHA-1 of the
     control member must match the index's `C:Q1...`, and the SHA-256 of
     the data member must match the `datahash` in the control's
     `.PKGINFO`. It then unpacks the data tar into `ROOT` and records the
     package in `ROOT/.get-alpine/pkgs/`.
   - It does not check the index signature: the download is HTTPS, and
     apk checks it next, with the keys just unpacked.
   - It writes the root's `/etc/apk/repositories` (the mirror and branch
     of this run) and an `/etc/resolv.conf` (see Sockets).
   - A dropped download resumes where it stopped, or starts over from a
     server that cannot resume. Three attempts in a row that get no
     further give up.
2. **apk.** `get-alpine` then runs `linux --root ROOT apk add --initdb
   --no-progress BASE... PACKAGE...`. `BASE` is the bootstrap set, so
   apk's world lists it from the start. `--initdb` makes the database on
   the first run and does nothing afterwards. apk:
   - resolves the dependencies;
   - downloads the packages, with musl's networking over the layer;
   - checks the repository signatures and the package checksums;
   - unpacks the packages and runs their install scripts and triggers:
     busybox's applet links, GSettings schemas, gdk-pixbuf loaders, the
     MIME database, users and groups (what a scriptless unpack would
     have to do by hand);
   - records them in `ROOT/lib/apk/db` and `ROOT/etc/apk/world`.

   On the first run apk installs the bootstrap set once more, over the
   unpacked files, so its database owns it too. An installed package
   costs nothing. `-u` (`--update-cache`) refreshes the indexes first.
   From then on `linux --root ROOT apk ...` is the whole package manager:
   `add`, `del`, `upgrade`, `info`, `fix`.
3. **`-n`** leaves apk out. The packages asked for are fetched, checked
   and unpacked natively, as the bootstrap set is. Their install scripts
   do not run, and apk's database does not know them. The layer need not
   be loaded. `linux-compat/alpine-disk.sh` uses `-n` on the host, which
   has no layer to run apk under.

Alpine keeps `/lib` and `/usr/lib` separate, so no symlinks are needed.
The dynamic linker is `/lib/ld-musl-<arch>.so.1`.

### Running a program in a root

`linux --root ROOT` chroots into `ROOT` (the native `chroot`) before the
exec, with a Linux `PATH`. The program finds its dynamic linker, shared
objects and data files at their Alpine paths.

Before the chroot, the launcher bind-mounts the system's `/dev`, `/proc`
and `/net` into `ROOT` (`mount SRC TARGET bind`), so the chrooted process
sees them like any other directory. Binds last until reboot or `umount
TARGET`, and binding a target again replaces the bind.

### Mirror, branch and root

- `ALPINE_MIRROR` overrides `https://dl-cdn.alpinelinux.org/alpine`.
- `ALPINE_BRANCH` overrides `latest-stable`.
- The default root is `/data/alpine` when the boot disk's data partition
  is mounted at `/data` (`docs/install.md`). It is kept across reboots
  and upgrades.
- Otherwise the default root is `/tmp/alpine`. `/tmp` is a tmpfs in the
  kernel heap, so a root there holds a few small packages and is gone at
  reboot.

`etc/policy` labels `/data/alpine` `sys.pkg`, as it labels `get-myos`'s
apps. A user's shell reads and runs what is there, and the administrator
installs it.

### Big roots

A root too big for the data partition goes on another disk. For
instance, Alpine's Rust compiler (rust, LLVM and gcc: ~300 MB of
downloads, ~600 MB installed) on the scratch disk of a test boot:

```sh
mkfs.ext2 /dev/nvme1n1/data && mount /dev/nvme1n1/data /mnt ext2
run-myos get-alpine -r /mnt/alpine rust
linux --root /mnt/alpine rustc --version
```

Under emulation that download takes a couple of hours. The mirror drops
long transfers, and a package whose transfer apk loses is installed by
the next run.

Making the same root on the host is quicker:

```sh
linux-compat/alpine-disk.sh ARCH OUT.img [NAME=]PACKAGE[,...]...
```

- It builds get-alpine for the host (`-DALPINE_ARCH`).
- It unpacks the packages for ARCH into a directory with `-n`: their
  install scripts do not run, and apk's database in that root is empty.
- `NAME=` makes a root `/NAME`; one image can hold several.
- It makes an ext2 image of the directory, as big as its content, to
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

The script:

1. installs Alpine's rust, cargo, rust-src, lld, clang, bash, busybox and
   git into `DIR/alpine` with `get-alpine` (installed with `get-myos`
   first);
2. clones (or updates) the source;
3. builds `core`, `alloc` and `compiler_builtins` once for
   `x86_64-unknown-none` into Alpine's `rustlib`. Alpine ships them for
   its own target only. The kernel's `build.rs` builds every module and
   user program in a target directory of its own;
4. runs `cargo build -p kernel` under `linux --root`.

Each step is skipped when its result is already there. The scratch disk
needs a few GiB. Build scripts are linked with clang.

Some programs are not built there yet, and are copied from the running
system instead: the C and `std` programs the kernel embeds (the std
demos, c-hello, oksh, getty, login) and `http`, whose TLS library is
LibreSSL over newlib. `MYOS_PREBUILT=http` has `kernel/build.rs` take it
as it is.

The kernel it builds boots: with it in the image instead of the host's,
`cargo run -- test-mini` passes. Under TCG a first run takes hours:
`core` and `alloc` about 25 minutes, each module or user program one to
three, the kernel crate itself an hour.

Known gaps:

- `http` (LibreSSL and newlib: autotools, `make`, `python3`) and the other
  C programs are not built in myos.

## How a process becomes a Linux process

Linux musl binaries and myos newlib binaries are both plain SYSV
static-PIE ELFs, so the kernel does not guess. A process has the **Linux
personality** only if:

1. `linux PROGRAM ...` (`linux-compat/launcher.c`, a native program) calls
   `SYS_LINUX_NEXT_EXEC` and then execs `PROGRAM`. The next successful
   exec starts the image with the Linux personality; or
2. a process that already has it calls Linux `execve`.

`fork` inherits the personality, and a native exec clears it. Everything
else (the shell, init, every port) is untouched.

A native myos binary does not work under `linux`: its myos syscalls get
decoded as Linux ones. It is killed by its own fault, and the kernel stays
up.

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

A *personality* is a foreign syscall ABI that a module provides. One can
be registered (`KernelApi::personality_register`, ABI 13). The kernel
keeps two bits per task slot, *active* and *pending*. It calls the
module's `PersonalityOps` at the points below. All of them are generic:
nothing in the kernel is Linux-specific.

- **Syscalls** (`syscall_dispatch`). An active task's syscalls go to
  `ops.syscall(nr, a0, a1, a2, regs)` instead of the native table. The
  usual signal handling at syscall exit follows. `regs` is the saved user
  register block, so the module reads further arguments and sets the
  result the arch way.
- **Spawn, fork, thread creation and exec** (`task/lifecycle.rs`,
  `task/thread.rs`). The kernel resets, copies or applies the bits, and
  calls `ops.on_spawn / on_fork / on_thread / on_exec`. The module does
  the same with its own per-slot state: fd paths, the trampoline, the
  `clear_tid` word.
  - `on_spawn`, `on_fork` and `on_thread` run with the core's task lock
    held and interrupts off. So a lock the module takes in them is never
    held while calling into the core. `current_pid` takes that lock, so
    `files.rs` reads the pid before locking its tables.
  - The core switches the thread pointer and the FP/SIMD registers for
    every user task (`task/tp.rs`, `task/fpu.rs`).
- **Signal delivery** (`signal.rs`). A caught signal of an active task
  goes to `ops.deliver(regs, &SignalDelivery, &mut ret)`. `SignalDelivery`
  holds the decided handler, the trampoline, the return context and the
  native mask the handler's return restores. On x86_64 its `arch` field
  carries the user code segment. The module writes the foreign frame or
  fails; on failure the kernel kills the task with `SIGSEGV`.
- **riscv64 user entry** (`arch/riscv64/user.rs`, `user_sstatus`). Every
  task runs with the FPU on (`sstatus.FS`), Linux's (rv64gc) and native
  ones alike.
- **Exec** (`user/syscall.rs`).
  - The new image gets the SysV auxv entries (`AT_PHDR`, `AT_PHNUM`,
    `AT_ENTRY`, `AT_BASE`, ...) and the ids, all 0. A native image gets
    them too. Without `AT_UID`/`AT_EUID`/`AT_GID`/`AT_EGID`, musl runs as
    setuid and ignores `LD_LIBRARY_PATH`.
  - With the personality pending or active, the kernel maps the image's
    `PT_INTERP` dynamic linker (see Dynamic linking).
  - `KernelApi::personality_exec` is the exec that keeps the personality
    (Linux `execve`).
  - `SYS_LINUX_NEXT_EXEC` sets the pending bit for the launcher.

The module's other needs are plain `KernelApi` services, added with
ABI 13:

- `native_syscall`: a native syscall from kernel mode, e.g. `read`,
  `brk`, `waitpid`;
- `copy_from_user`;
- the path and VFS calls (`path_resolve`, `vfs_stat`, `vfs_listdir`, ...);
- fd calls (`fd_pread`, `fd_kind`, `fd_dup2`, `pipe_open`, ...);
- `mmap`;
- the signal table (`signal_get_action` / `signal_set_action`, masks,
  `signal_kill`, `signal_sigsuspend`, ...);
- `fpu_save` / `fpu_restore`, the thread pointer;
- `thread_spawn_from`: a thread resuming like the caller of a syscall on
  a new stack, for `clone`;
- `wait_addr` / `wake_addr`, for `futex`;
- `task_sleep_until`, `wall_time_us` and `rng_fill`.

ABI 25 added `vfs_set_times`. ABI 27 added `thread_place`: a `clone`d
thread's own CPU, once its ids are stored. The module uses `alloc` (`Vec`,
`String`) through the kernel heap (`KernelApi::alloc` / `dealloc`).

### Dynamic linking

A dynamically linked program names its interpreter in `PT_INTERP`. For
musl that is `/lib/ld-musl-<arch>.so.1`, which is `libc.so` itself. For a
Linux exec of such a program, the kernel:

1. reads the interpreter (`exec_interp` in `user/syscall.rs`) before
   replacing the current image, so a missing one fails the exec cleanly;
2. maps the program as usual but **without applying its relocations**
   (`elf::realize_as(.., relocate: false)`). They refer to symbols in
   shared objects, which the dynamic linker resolves;
3. maps the interpreter, also unrelocated (it relocates itself), at the
   start of the new image's `mmap` window, with per-segment protections.
   It records it as `mmap` regions, so fork shares it and exit frees it;
4. starts the interpreter, with `AT_BASE` set to its load address and
   `AT_PHDR` / `AT_ENTRY` naming the program.

The dynamic linker then loads the `DT_NEEDED` objects, and later the
`dlopen` ones, from `/lib`, `/usr/local/lib` and `/usr/lib`. It uses
`open`, `read`, `pread64` and `mmap` of the file, and maps each segment
over the span it reserved first. That needed this core `mmap` work, which
native programs share:

- **File-backed mappings.**
  - `MAP_PRIVATE`: the pages are a private copy of the file, never
    written back.
  - `MAP_SHARED`: the pages are the file's, the page cache's frames for
    them. Every process that maps them shares them, and so do `read` and
    `write`. `msync`, `munmap`, exit and exec write them back to the file.
    Through a read-only fd a shared mapping may not be written.
  - `MAP_SHARED | MAP_ANONYMOUS`: a tmpfs file no name reaches, shared
    with forked children (`fs::anon_file`). A tmpfs file is bytes in the
    kernel heap that the mapping is written back to, so a segment costs
    its size twice, up to 16 MiB (issue #338).
- **Demand paging.** A mapping takes no memory until it is used.
  - The first touch of a page gives it a frame, zeroed or read from the
    file (`user::fault_in`). A touch is a page fault from userspace or a
    kernel copy into a user buffer.
  - A page of a private file mapping is the page cache's frame for it,
    shared by every process that maps it (`fs/pagecache.rs`). It is
    mapped read-only, whatever the mapping's protection, until the first
    store to it. That store gives the process its own writable copy of
    the page (copy-on-write). The store is a protection fault from
    userspace, or the kernel copy's check. `mprotect` to writable copies
    the same way.
  - A fault on a file-backed page also maps the following pages of the
    region that the cache already holds, up to 16 in all (fault-around).
    They are mapped read-only; a store takes the mapping's protection as
    above. So from its second run on, a program's code and data are paged
    in with a few faults rather than one per page. `/proc/meminfo` counts
    the faults and the pages mapped around them.
  - A fork shares every page of the parent's the same way, copy-on-write
    (`README.md`).
  - For scale: rustc reserves 256 MiB for its allocator and maps some
    250 MiB of libraries, and `rustc --version` touches about 30 MiB of
    it.
  - A file changed or deleted while mapped gives its new contents (or
    zeros) to the pages not touched yet.
- `MAP_FIXED` replaces whatever is mapped in its range.
- `munmap` of any range in the `mmap` window (holes included) and
  `mprotect` of part of a mapping split the mapping.
- Free address space is reused (first fit) instead of only growing.
- A larger window, 64 GiB on every arch, because WebKit's JavaScript
  engine reserves 4 GiB aligned to 4 GiB; and 4096 mappings per process.

### Signal handlers

Delivery reuses the native decision path: masks, `SA_RESTART`, `EINTR`,
`sigsuspend` (`docs/signals.md`). Only the frame differs.

For a Linux task the kernel writes a Linux `rt_sigframe` below the
interrupted stack pointer. It holds a `siginfo_t` and a `ucontext` with
the interrupted registers, the blocked mask and the FP/SIMD state:

- x86_64: the FXSAVE image;
- aarch64: `fpsimd_context`;
- riscv64: the D state.

The kernel enters the handler as `handler(sig, &info, &uc)`, returning to
the restorer, which calls `rt_sigreturn`. musl passes `SA_RESTORER` on
x86_64 and aarch64. riscv64 has no `sa_restorer` (Linux uses its vDSO
there), so the kernel maps a one-page trampoline into the process on the
first such `rt_sigaction`.

A fault (an undefined instruction, a bad access) raises its signal at
once, from the trap's registers (`docs/signals.md`, Faults):

- the `siginfo_t` has its `si_code` and `si_addr`;
- the `sigcontext` has the address (`fault_address` on aarch64, `cr2` on
  x86_64);
- `rt_sigreturn` resumes the faulting instruction with every register the
  `ucontext` holds. On x86_64 that includes `rcx` and `r11`, because the
  kernel then returns with `iretq`.

On x86_64 the callee-saved registers (rbx, rbp, r12-r15) are reported as
0 in `uc_mcontext` and not restored from it. The handler preserves them,
and the kernel does not keep a per-task copy at syscall entry.

### Sockets

A Linux socket is a conversation of the native `/net` (`docs/net.md`,
`modules/linux/src/net.rs`). Its fd is the conversation's `data` file, so
reads, writes, `dup`, `fork` and `close` are ordinary fd operations, and
the last close hangs the conversation up.

- `socket` opens `/net/{tcp,udp}/clone`. The fd is the new conversation's
  `data`, and its link names it.
- `connect` writes `connect a.b.c.d!port` to the conversation's `ctl`. For
  an `AF_INET6` socket the peer is its IPv6 address, or for an
  IPv4-mapped one its IPv4 address, which it names `::ffff:a.b.c.d` back,
  as Linux does.
- Readiness comes from the conversation's `status` and the bytes waiting
  in `data`.
- The `/net` files never block. A blocking read or connect sleeps until
  netd's next reply wakes the pollers. A write retries while netd's
  request ring is full, or while a TCP conversation has no send room left
  (8 KiB queued in netd).
- A datagram socket's `sendto`, `sendmsg`, `recvfrom` and `recvmsg` go
  through the conversation's `hdata` (`docs/net.md`). Each datagram there
  carries its destination or its sender. So musl's resolver, which asks
  every nameserver from one socket and takes the first answer, gets the
  answer from whichever server sends it.
- `send`, `write`, `recv` and `read` on a datagram socket are the bare
  datagrams of `data`, to and from the peer `connect` set.

A chrooted process reaches `/net` through the bind `linux --root` sets
up. `get-alpine` gives a new root an `/etc/resolv.conf` for musl, naming
the DNS servers the system uses: the `dns=` lines of `/net/ndb`, or QEMU's
`10.0.2.3` when there are none. It is written once, when the root has
none, so a root moved to another network needs it edited or removed.

Under QEMU's user network, a server listed after the lease's server loses
that one's answers. QEMU keeps one host socket per guest source port, with
the last address sent to as its peer. It gives an answer the virtual
nameserver's address only while that peer is the nameserver. musl asks
every server at once, from one socket, and ignores an answer from an
address it did not ask.

Every root also gets Alpine's CA bundle (`ca-certificates-bundle`,
`/etc/ssl/certs/ca-certificates.crt`, where OpenSSL's `/etc/ssl/cert.pem`
points). Alpine has it from `alpine-base`, and `python3`, `curl` and
`wget` do not depend on it. Without it every TLS connection fails its
certificate check (`unable to get local issuer certificate`).

An `AF_UNIX` stream socket is a `/net/unix` conversation, mapped as
libgloss maps a native program's (`docs/sockets-unix.md`):

- `connect` writes `connect NAME`;
- `listen` announces the name `bind` gave;
- `accept` takes the next connection from the listener's `listen`;
- an abstract name (leading NUL) is `@name`.

Names live in `/net/unix`, not in the filesystem. So Linux and native
programs reach each other by name, also from inside a `linux --root`. An
Alpine X client connects to the native X server's `/tmp/.X11-unix/X0`
with nothing in its root: libxcb tries the abstract name first, is
refused, and takes the path.

Fds pass as `SCM_RIGHTS` messages through the conversation's `rights`.
Firefox's IPC sends its sockets and shared memory this way. A received fd
gets the layer's state back from its path: a unix socket's conversation,
a signal fd's translation, a file's path.

## Supported syscalls

Numbers differ per arch. x86_64 has its own table. aarch64 and riscv64
share the `asm-generic` one, which has only the `*at`, `clone`, `ppoll`
and `dup3` forms of the legacy calls.

Anything not listed here returns `ENOSYS`. `/proc/linux_trace` reports
each such call (see Debugging).

### Files

`read`, `write`, `readv`, `writev`, `pread64`, `pwrite64`, `pwritev(2)`,
`open`, `openat`, `close`, `stat`, `lstat`, `fstat`, `newfstatat`,
`lseek`, `access`, `faccessat`, `pipe`, `pipe2`, `dup`, `dup2`, `dup3`,
`flock`, `truncate`, `ftruncate`, `fsync`, `fdatasync`, `chmod`, `chown`
(and their `f`, `l`, `at` forms), `getcwd`, `chdir`, `fchdir`,
`mkdir(at)`, `rmdir`, `unlink(at)`, `rename(at/at2)`, `symlink(at)`,
`readlink(at)`, `utimensat`, `utimes`, `poll`, `umask`. Also:

- `statfs`, `fstatfs`: no filesystem magic in `f_type`.
- `getdents64`: a directory fd reads the listing its first read took,
  whole, until it is rewound. Entries removed while it is read do not
  shift the rest.
- `ioctl`: the tty requests from the terminal's ctl file (`docs/tty.md`),
  `FIONBIO` on sockets, and the console keymap and module devices
  natively.
- `fcntl`: dup, fd flags, record locks.
- `eventfd(2)`, `timerfd_create`, `timerfd_settime`, `timerfd_gettime`,
  `epoll_create(1)`, `epoll_ctl`, `epoll_wait`, `epoll_pwait(2)`,
  `signalfd(4)` (see Limits for what backs them).
- `fallocate`: mode 0, which is what `posix_fallocate` asks. The file
  grows to the range's end, and its blocks are not reserved ahead. Other
  modes are `EOPNOTSUPP`.
- `readahead`: a hint, done.

### Memory

- `brk`, `munmap`, `mprotect`, `msync`.
- `mmap`: anonymous, shared anonymous, and private and shared file
  mappings.
- `madvise`: `MADV_DONTNEED` drops the pages. They read as zero, or as
  their file, next time, as allocators such as rustc's Scudo expect.
  Other advice is ignored.
- `mremap`: a mapping shrinks in place. It cannot grow (`ENOMEM`); musl's
  `realloc` then allocates anew and copies (issue #421). A range that is
  not mapped is `EFAULT`, which ends musl's `pthread_getattr_np` walk down
  the main thread's stack.

### Processes

- `fork`, `vfork` (as fork), `clone` (see Threads).
- `execve`. A `#!` script runs its interpreter, with the line's one
  optional argument, the kernel's way: `exec` on the script and its
  `exec` rule apply (`docs/security.md`). A file that is neither an ELF
  nor a script fails with `ENOEXEC`.
- `exit` (the thread), `exit_group`, `wait4`.
- `waitid`: `P_ALL` and `P_PID`; `WNOHANG` and `WNOWAIT`; exits only.
- `kill`, `tkill`, `tgkill`, `getpid`, `gettid`, `getppid`, `getpgid`,
  `setpgid`, `getpgrp`, `getsid`, `setsid`, `sethostname`, `arch_prctl`,
  `set_tid_address`, `set_robust_list`, `prlimit64`, `getrlimit`,
  `sched_yield`.
- `uname`: the node name is the kernel's host name
  (`/proc/sys/kernel/hostname`).
- `sysinfo`: uptime and memory from `/proc/meminfo`. No swap, load or
  process count.
- `reboot`: the native `power` (`docs/power.md`). Turning Ctrl-Alt-Del on
  or off does nothing.
- `get/set uid/gid`, `getresuid`/`getresgid`: uid and gid are 0 whoever
  runs the program. What it may do is the security policy's
  (`docs/security.md`).
- `getpriority` reports nice 0. `setpriority` is accepted; the core
  schedules.
- `prctl`:
  - `PR_SET_NAME` and `PR_SET_VMA` are accepted and not kept;
  - `PR_GET_DUMPABLE` reports set;
  - `PR_SET_NO_NEW_PRIVS` is accepted and not done;
  - `PR_SET_PDEATHSIG` is the process's, not the parent thread's as on
    Linux. The core sends it when the parent process ends, however it
    ends. It is kept across exec, not across fork. A real-time signal is
    `EINVAL`;
  - any other option, seccomp included, is `EINVAL`.
- No namespaces: `unshare` of anything and `clone` with a `CLONE_NEW*`
  flag are `EINVAL`. bubblewrap says so, and glycin, WebKitGTK's image
  loader, runs its loaders without a sandbox.
- `capget` reports no capabilities, and `capset` grants none.
- `/proc/sys/kernel/overflowuid` and `overflowgid` read 65534.

### Threads

`clone` with `CLONE_THREAD` starts a native thread (`docs/threads.md`)
that resumes like a forked child on its new stack. It takes `CLONE_VM`,
`CLONE_FS`, `CLONE_FILES` and `CLONE_SIGHAND`, and `CLONE_SETTLS`,
`CLONE_PARENT_SETTID`, `CLONE_CHILD_SETTID` and `CLONE_CHILD_CLEARTID`.

`futex` maps onto the core's `wait_addr`/`wake_addr`:

- `WAIT`/`WAKE`, and the `_BITSET` forms, with the bitset taken as "all";
- `REQUEUE`/`CMP_REQUEUE`, as a wake of every waiter.

A thread's `exit` clears and wakes its `CLEAR_TID` word, which is what
`pthread_join` waits on.

`clone` without `CLONE_THREAD` is one of:

- the fork form;
- musl's `vfork` on aarch64 and riscv64: `CLONE_VM | CLONE_VFORK` on the
  parent's stack. It is a fork, as the x86_64 `vfork` syscall is;
- `posix_spawn`'s: `CLONE_VM | CLONE_VFORK` with a stack, which Rust's
  `Command` uses through musl. It is a fork whose child starts on that
  stack (the core's `fork_from`).

The child's memory is a fork's (the parent's pages shared copy-on-write,
`README.md`) rather than shared. posix_spawn and a `vfork` child that only
execs do not notice: posix_spawn's child reports a failed exec through a
pipe the parent waits on.

### Signals

- `rt_sigaction`: handlers with `SA_SIGINFO`, `SA_RESTART`, `SA_NODEFER`,
  `SA_RESETHAND` and `sa_mask`.
- `rt_sigreturn`, `rt_sigprocmask`, `rt_sigpending`, `rt_sigsuspend`,
  `rt_sigtimedwait`.
- `sigaltstack`: no alternate stacks.

The layer translates signal numbers and masks between Linux and the
native (newlib) numbering, also in `wait4` statuses.

### Socket calls

See Sockets above for how they work.

- `socket`: `AF_INET` stream and datagram, `AF_UNIX` stream.
- `socketpair` (`AF_UNIX`).
- `connect`, `bind`, `listen`, `accept`, `accept4` (`AF_UNIX`).
- `sendto`, `recvfrom`, `sendmsg`, `recvmsg`. On a unix socket they carry
  fds as `SCM_RIGHTS` (`docs/sockets-unix.md`).
- `shutdown`, `getsockname`, `getpeername`.
- `getsockopt`:
  - `SO_ERROR`, `SO_TYPE`;
  - `SO_SNDBUF`/`SO_RCVBUF`: 64 KiB for a unix socket, 8 KiB otherwise;
  - `SO_PASSCRED`;
  - `SO_PEERCRED` gives the caller's own pid, and uid and gid 0;
  - `SO_PEERSEC`, `SO_PEERGROUPS` and `SO_PEERPIDFD` are `ENOPROTOOPT`, as
    on a kernel without them. zbus would take an answer of 0 for a pidfd
    and close fd 0.
- `setsockopt`: `SO_PASSCRED` on a unix socket makes each read come with
  the sender's pid as `SCM_CREDENTIALS` (`/net/unix`'s `from`,
  `docs/sockets-unix.md`). Anything else is accepted and ignored.
- `read`/`write`, `poll`, `fstat` and `ioctl(FIONBIO)` work on sockets
  too.

### Time

- `clock_gettime`:
  - `CLOCK_REALTIME`, its `_COARSE` and `_ALARM` forms, and `CLOCK_TAI`
    give the time of day;
  - `CLOCK_MONOTONIC`, its `_RAW` and `_COARSE` forms, and
    `CLOCK_BOOTTIME` give the kernel's time since boot, which setting the
    date does not move;
  - the process's and the thread's CPU-time clocks give their CPU time;
  - another id is `EINVAL`.
- `clock_getres`: a microsecond for the time of day, a nanosecond for
  the others.
- `gettimeofday`, `time`, `nanosleep`.
- `getrusage`: the process's, the thread's or the reaped children's CPU
  time, all as user time. The other fields are 0.
- `clock_nanosleep`: an absolute time is on its clock, the time of day or
  the time since boot. `FUTEX_WAIT_BITSET`'s are the same, with
  `FUTEX_CLOCK_REALTIME` the time of day.

### Other calls

- `getrandom`.
- `memfd_create`: a file of `/dev/shm`, created and unlinked at once, as
  libgloss's. `F_ADD_SEALS` and `F_GET_SEALS` keep and report a memfd's
  seals, from any fd naming it, and do not enforce them. The last 256
  sealed memfds are remembered.
- `poll` and `select` (x86_64), `ppoll` and `pselect6`, which FLTK's event
  loop waits in (dillo). They share one readiness:
  - readable on `POLLIN`, `POLLHUP` or `POLLERR`;
  - writable on `POLLOUT` or `POLLERR`;
  - no exceptional conditions;
  - the signal mask is not applied, and `select` does not update its
    timeout.
- `sched_getaffinity` reports every online CPU, so cargo and rustc run
  their jobs in parallel. `sched_setaffinity` is accepted; the core
  places tasks.
- `fadvise64`: a hint, accepted.
- `membarrier`: the global and the private expedited barriers are one and
  the same. Every CPU running a task is interrupted and answers, as for a
  TLB shootdown, before the call returns. Registering is accepted.
- The extended attribute calls (`listxattr`, `getxattr`, `setxattr`,
  `removexattr` and their `l` and `f` forms). No myos file system keeps
  attributes. So, as on a Linux file system without them, a file lists
  none, reading one is `ENODATA`, and setting or removing one is
  `EOPNOTSUPP`. Python's `shutil.copy2` (so `ensurepip` and pipx) copies
  and skips that.

## Limits

- **Binaries.** musl binaries only, static or dynamically linked,
  position-independent or not. No glibc ones: not tried, and glibc needs
  more syscalls.
  - A position-independent program loads at 1 GiB.
  - A non-PIE (`ET_EXEC`) program loads at its link address. Alpine's gcc
    links at 0x400000 on x86_64, 0x10000 or 0x400000 elsewhere. The
    address must leave the first 64 KiB unmapped, and leave room above
    the image for the stack, the heap and the mmap window in the user
    window: `[0, 512 GiB)` on x86_64, `[0, 66 GiB)` on aarch64 and
    riscv64.
- **Signals** act at syscall exit, as for native programs. A task looping
  in user mode is not interrupted until its next syscall. Only `SIGKILL`
  acts on the interrupt return, and a fault's signal acts at once. No
  alternate signal stacks, and no real-time signal queueing.
- **`clone`.** The only `clone` with `CLONE_VM` but not `CLONE_THREAD` is
  posix_spawn's.
- **No System V IPC** (`shmget`, `semget`, `msgget`). A Linux program has
  POSIX shared memory: `shm_open`, `memfd_create`, `MAP_SHARED`.
- **Close-on-exec and non-blocking fds.** Close-on-exec (`O_CLOEXEC`,
  `FD_CLOEXEC`, `FIOCLEX`) and `O_NONBLOCK` outside sockets are flags the
  layer keeps per fd (`files.rs`). A successful exec closes the
  close-on-exec fds (the `on_exec` hook). A non-blocking pipe end that
  would block gives `EAGAIN`, from the core's pipe readiness, as libgloss
  does for native programs.
- **Event fds.** The fd the program holds is the native one, read,
  written and polled by the core:
  - an eventfd is the kernel's event counter (`/dev/event/N/data`,
    `docs/events.md`);
  - a timerfd is its timer (`/dev/timer/N/data`);
  - an epoll fd is its wait set (`/dev/wait/N/data`);
  - a signalfd is a kernel signal fd (`/dev/sig/N/data`). The layer flags
    the fd and turns each 16-byte record a read gives into a `struct
    signalfd_siginfo`, with `ssi_signo` Linux's number and the rest 0.

  `eventfd2`'s semaphore mode and initial value, `timerfd_settime` /
  `timerfd_gettime` and `epoll_ctl` are the object's `ctl` and `status`.
  `epoll_wait` waits for the set's readiness and takes the ready members
  (`KernelApi::wait_take`). `EPOLLET` reports a readiness once, and again
  only once the fd's object changed (`docs/events.md`). `EPOLLPRI`,
  `EPOLLRDHUP`, `EPOLLEXCLUSIVE` and the signal mask of `epoll_pwait` are
  ignored. A `TFD_TIMER_ABSTIME` value is on the clock the timerfd was
  created on: the layer remembers the timers made on the wall clock and
  arms them with `realtime`.
- **`listen` and `accept`** serve `AF_UNIX` sockets only. A TCP socket's
  `listen` is `EOPNOTSUPP` (issue #415).
- **`AF_UNIX` sockets** are `/net/unix` conversations (see Sockets).
  `socketpair(AF_UNIX, SOCK_STREAM | SOCK_SEQPACKET)` is a `/net/unix`
  pair, its ends kept like TCP sockets; Rust's `Command` reports a failed
  exec through one. A `SOCK_SEQPACKET` socket's conversation keeps
  records (`docs/sockets-unix.md`): a write, a `writev` or a `sendmsg` is
  one record, and a read returns one (`MSG_TRUNC` when it was longer). A
  record past 8 KiB is `EMSGSIZE`. No `SOCK_DGRAM`.
- **File locks** are the native ones (`README.md`, "File locks"): `flock`,
  `fcntl`'s record locks (`F_SETLK`, `F_SETLKW`, `F_GETLK`) and the open
  file description's (`F_OFD_SETLK`, ...). So cargo, SQLite and git guard
  against each other as on Linux.
- **Files.**
  - `ftruncate` and `O_CREAT|O_EXCL` are native too: any length, and the
    name checked and created in one step.
  - `O_NOFOLLOW` and `O_DIRECTORY` are kept (`ELOOP`, `ENOTDIR`).
  - `link` and `linkat` fail with `EPERM`: myos has no hard links.
  - `pwrite` past the end of a file leaves zeros in the gap.
  - `chmod`, `chown` and `fsync` succeed and do nothing; so do `lchown`
    and the `at` forms with `AT_SYMLINK_NOFOLLOW` on a dangling link.
    What a program may do to a file is the security policy's
    (`docs/security.md`), and ext2 writes a file back when its last fd
    closes.
  - `stat`'s permission bits are the owner's only, and say what the
    calling program may do. The layer's file calls are checked like the
    native ones.
- **File times.** `stat` reports a file's access and modification times
  in whole seconds where the filesystem keeps them, tmpfs and ext2.
  `st_ctime` is the modification time. The read-only and device
  filesystems (`/bin`, `/dev`, `/proc`, ...) and FAT report 0.
  `utimensat`, `futimens` and `utimes` set the times there (`UTIME_NOW`,
  `UTIME_OMIT`; nanoseconds are dropped) and fail with `EROFS` elsewhere.
- **Sockets.** IPv4 and IPv6 clients and `AF_UNIX` streams only: no
  `AF_INET` or `AF_INET6` `listen`/`accept`.
  - No half-close: `shutdown` hangs up only for `SHUT_RDWR`.
  - A TCP socket's local address is reported as the any address, port 0.
    A UDP one's is netd's, which musl's getaddrinfo uses to sort IPv6
    addresses against IPv4 ones (RFC 6724).
  - Options are ignored, `IPV6_V6ONLY` too: an `AF_INET6` datagram
    socket's port is on `::`, IPv4's too.
  - `poll` reports a connected socket writable even with no send room
    left; a nonblocking write then says `EAGAIN`.
- **The native limits apply:**
  - exec: up to 1024 arguments and 1024 environment strings, at most
    128 KiB together.
  - A dynamically linked, position-independent program (most of
    Alpine's) is mapped from its file like a shared object. Its dynamic
    linker is loaded as the image, and the program's segments become mmap
    regions paged in on first touch. So only the mmap window bounds its
    size (Alpine's cargo is 23 MB).
  - Any other program is read whole: a file of at most 16 MiB from a
    writable filesystem (the initramfs has no limit), whose loaded image
    spans at most 4096 pages (16 MiB).
  - A native position-independent program past either limit (GTK's,
    statically linked) is mapped from its file the same way, over an
    empty one-page image, and the kernel relocates it in place. The pages
    its relocations write become the process's own copies. That happens
    after the old image is gone, so a program the kernel cannot map or
    relocate (a text relocation, into a read-only page) exits with status
    127 and a `[ FAIL ] exec PATH: ...` console line naming why.
  - A per-process `mmap` window of 64 GiB on every arch, above the image,
    its stack and heap (from 1 GiB for a position-independent program),
    with at most 4096 mappings. Adjacent mappings with the same protection
    and backing are merged. musl's malloc makes hundreds of small
    neighbouring ones: jq peaks at 188, and rustc's Scudo allocator needs
    more than 256 to compile `core`.
  - At most 1024 distinct files are mapped at once; Firefox and its GTK
    stack map well over a hundred. A file mapping past that is read in
    whole when it is made.
  - A 16 MiB `brk` heap.
  - 256 fds per process, 2048 open file descriptions in the system, 512
    tasks (threads and processes) in total, and 64 pipes in the system
    (and 24 named FIFOs), each with a 4 KiB buffer.
  - `/tmp` (tmpfs) files of at most 128 MiB each, all of them in the
    kernel heap. The heap is a quarter of the memory, 64 MiB to 1 GiB and
    at most half its largest region: 1 GiB in the 4 GiB CI guests. The
    boot's `[ OK ] heap: N MiB` line says how much.
  - A full filesystem refuses the write that does not fit with `ENOSPC`,
    after the one that filled it took what fit. `/data` is 64 MiB on a
    boot disk written from the image (`docs/install.md`).

## Debugging

A program that exits without a word usually asked for something the
layer does not have, or crashed. These tools say which.

**The trace.** `echo 1 > /proc/linux_trace` makes the layer report on the
console every syscall it does not implement, as it answers `ENOSYS`. Each
line has the process and the first three arguments, in the arch's own
numbering (x86_64's, or the generic table of aarch64 and riscv64).
Writing `/proc` needs root: it is the policy's `proc {all}`
(`docs/security.md`).

```
linux: pid 7 syscall 4000 (0x1, 0x2, 0x3): ENOSYS
```

`echo 0 > /proc/linux_trace` turns it off. It starts off.

**What a hung or spinning program is doing.** `echo 2 >
/proc/linux_trace` also keeps every thread's last 8 syscalls, with their
answers and its count of calls, without printing them. Each write
publishes what was kept so far, after the level. So with the program
running in the background, write `2` again and read the file:

```
2
tid 9 pid 9: 1520 calls
  202 (0x7f1c0, 0x80, 0x0, 0x0) ...
tid 11 pid 9: 4800213 calls
  24 (0x0, 0x0, 0x0, 0x0) = 0x0
```

- The thread whose count climbs between two reads is the busy one.
- A call with `...` is still waiting. (`202` is x86_64's `futex`, `24` its
  `sched_yield`.)
- A thread spinning without syscalls keeps its count still. Its last
  calls show what it last asked for.
- Each write keeps its copy of the text, under 1 KiB per thread, because
  procfs files do not give theirs back.

**Where a fault was.** The kernel's `user fault` line names the file and
the ELF virtual address of the faulting instruction when it is in a file
mapping, which is where a shared library's code is:

```
[ WARN ] user fault: page fault in /lib/libsmoke.so+0x15a0 cr2=0x0 rip=0x80011bb5a0 ...
```

- The address is what `nm`, `objdump --start-address` and `addr2line -e
  FILE` take, on a copy of the same file on the host.
- The dynamic linker (musl's libc too) and the last page of each segment
  of a program are copied at exec, and say `in anonymous memory` (issue
  #418).
- A write to address 0 (`cr2=0x0`, `code=0x6` on x86_64) from a program's
  own code is often deliberate. Firefox's `MOZ_CRASH` and
  `MOZ_RELEASE_ASSERT` store a reason and write to 0; the reason is the
  string the code loads just before the write.

## Testing

The boot tests are in `user/tests/kernel.sh` (`docs/testing.md`), tagged
`linux`. The test programs are plain musl C, so they can also be run on a
Linux host for reference.

### With the feature

When the image was built with `--features linux_compat`, every arch runs:

- `linux /bin/linux/linux-smoke`, expecting `LINUX-SMOKE OK`. It covers
  files, directories, mmap, fork/execve/wait4, pipes, Linux signal
  numbers, handlers, masks, `sigwait`, `EINTR` and `SA_RESTART`, and
  pthreads: a mutex, a condition variable, thread-local storage, thread
  ids, join, and `exit` from a thread ending the process.
- `linux /bin/linux/linux-dyn`, expecting `LINUX-DYN OK`. It covers a
  call, shared data, a relocated function pointer and a thread-local in
  `libsmoke.so`, `printf` from `libc.so`, and `dlopen`/`dlsym` of
  `libsmoke2.so`.
- `linux --root` of a bare root with the smoke's event fd checks
  (`linux-smoke evfd`, `LINUX-EVFD OK`), which run through the root's bind
  of `/dev`.
- In a bare root whose `resolv.conf` names the system's nameserver and
  then a dead address, musl's resolver (`linux-smoke dns`, `LINUX-DNS
  OK`) gets the answer from the live one.
- `linux_trace` turns the trace on, checks that a syscall no arch has
  still gets `ENOSYS` (`linux-smoke enosys`), and turns it off again.

The smoke catches its own faults (`SIGILL`, `SIGSEGV`, a handler that
returns to the store). A fault a program does not catch cannot be a boot
test, because the host takes the kernel's `user fault` line for a crash.
So the fault's location is not tested in the boot.

The full list then runs the Alpine tests:

- `run-myos get-alpine jq && linux --root /data/alpine jq -Rrn '...'
  /proc/mounts`. The root is the default one, on the test disk's data
  partition: bootstrapped by get-alpine, installed by apk under the layer
  and recorded in `/data/alpine/lib/apk/db`. jq counts the binds of
  `/dev`, `/proc` and `/net` it sees in the root, and the test expects
  `ALPINE-JQ 6`. It needs the Alpine mirror to be reachable.
- From the Python root of the Alpine disk (below; python3 and its 19
  dependencies, ~45 MB, mounted at `/tmp/alpine/python`):
  - `python3 -c 'import json,sqlite3;print("PYTHON",json.loads("[42]")[0])'`
    exercises the standard library and two C extension modules, expecting
    `PYTHON 42`;
  - with `urllib`, it resolves `github.com` (DNS over UDP) and fetches the
    launcher's test server's `http://10.0.2.2:8766/hello.txt` (TCP,
    `docs/testing.md`), expecting `HTTP 200`. It needs `github.com`'s name
    to be reachable;
  - an asyncio script (`alpine_python_asyncio`) is the end-to-end test of
    the event fds (`docs/events.md`). Its selector is `select.epoll` over
    `/dev/wait`, its signal handler is a wakeup fd in it, and its
    subprocess is watched from a thread. It runs an echo over an
    `AF_UNIX` socket, a timer, a timeout, `SIGUSR1` handled by the loop
    and a child's output, expecting `ASYNCIO EpollSelector ASYNCIO True
    True USR1 sub 0`.
- An X client (`xdpyinfo`, from the disk's `/x11` root) on the native X
  server.
- `rustc --version` from the disk's `/rust` root, expecting `rustc 1.`.

The Alpine disk is `target/alpine-ARCH.img`. `linux-compat/alpine-disks.sh
ARCH` makes it on the host: `alpine-disk.sh` with the three roots, using
`get-alpine -n`, so no install script runs, which rustc, Python and
xdpyinfo do without. `docs/testing.md` says how CI caches it. It is
attached as `/dev/nvme2n1/data`, and writes go to a QEMU snapshot.

### Without the feature

Without the feature (the quick list, and a CI boot whose selection has no
`linux` test), the test instead expects the module in `/proc/modules`,
loaded by the boot script's default `rc.local` with `insmod`, and a second
`insmod` to be refused. The module is built and loadable in every build.

### CI

- CI builds the musl pieces when their inputs change (`linux-compat` in
  `ci-ports.yml`). They are cached in the GHCR registry by
  `scripts/ci-registry.sh`, under a hash of `linux-compat/` and newlib,
  and packed into `ci-build.tar.zst`.
- A boot whose selection has a `linux` test builds its disk images with
  the layer (`MYOS_CI_FEATURES=linux_compat`; `docs/testing.md`, "In
  CI"). The full boot (on master, daily, or `full_boot` on dispatch) does
  so for all four disk images, so bios, uefi, aarch64 and riscv64 run the
  tests above. There are no separate Linux jobs.
- On master the **iso** job builds the x86_64 hybrid ISO with `--features
  linux_compat`, so the downloadable ISO always carries the layer, with
  the module loaded at boot. `get-alpine` comes from the release's
  packages. The test programs are not in it (they are the tests
  module's).

### Building the musl pieces

`linux-compat/build.sh` builds musl (static and shared) with clang for
each target. It skips itself when its outputs match its stamp,
`target/.myos-linux-compat-version`.

- The riscv64 binaries are built for Alpine's ISA, `rv64gc` with hard
  float. It is named explicitly because clang's default for a Linux
  target follows the host distribution: Ubuntu 26.04's emits RVA23 vector
  instructions, which QEMU's default CPU lacks.
- On aarch64 and riscv64, musl's `long double` is 128-bit and needs
  compiler-rt's quad-float builtins. The script fetches those sources
  (a pinned LLVM tag) and links them into `libc.so`. The static smoke test
  is not linked against them, so it (and the launcher) avoid `printf`.
