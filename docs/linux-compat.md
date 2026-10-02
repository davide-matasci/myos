# Linux syscall compatibility layer (optional)

> **Optional, off by default.** None of this is in a default build: the
> kernel code is behind a Cargo feature, and nothing about native myos
> programs changes when it is on. myos's own syscall ABI stays the primary
> interface; this layer is an add-on for running some unmodified Linux
> binaries.

With it, a Linux binary linked against musl (x86_64, aarch64 or riscv64),
static-PIE or dynamically linked, runs on myos when started through the
`linux` launcher:

```sh
cargo run --features linux_compat          # build + boot the x86_64 image
cargo run --features linux_compat -- aarch64   # (or riscv64)
# in the guest:
linux /bin/linux/linux-smoke               # prints LINUX-SMOKE OK
linux /bin/linux/linux-dyn                 # dynamic: prints LINUX-DYN OK
get-void jq                                # Void Linux packages (x86_64, aarch64)
linux --root /tmp/void jq -n '1+1'
```

## Void Linux packages: `get-void`

Nothing from Void is in the image: `get-void` (`linux-compat/get-void.c`, a
native program built with the layer on x86_64 and aarch64, the arches Void
has musl repositories for) downloads packages at run time with the guest's
`curl`:

```sh
get-void [-r ROOT] [-u] PACKAGE...         # ROOT defaults to /tmp/void
linux --root ROOT PROGRAM [ARG...]
```

1. The repository index (`<arch>-repodata`, a zstd-compressed tar holding a
   20 MB `index.plist`) is downloaded once and reduced, streaming, to
   `ROOT/var/db/get-void/index`: one line per package with its pkgver,
   sha256 and `run_depends`. `-u` refreshes it.
2. Each package and, recursively, its dependencies (`musl` included) is
   downloaded, checked against the index's sha256 and unpacked (zstd + tar)
   into `ROOT`. Already installed packages (`ROOT/var/db/get-void/pkgs/`)
   are skipped. INSTALL/REMOVE scripts are not run.
3. `ROOT` gets the directories and symlinks Void's `base-files` provides
   (`/lib -> usr/lib`, `/usr/lib64 -> lib`, ...): Void's dynamic linker is
   `/usr/lib/ld-musl-<arch>.so.1 -> /usr/lib64/libc.so`.

`linux --root ROOT` chroots into `ROOT` (the native `chroot`) before the
exec, with a Linux `PATH`, so the program finds its dynamic linker, shared
objects and data files at their Void paths. A chrooted Linux process still
sees the system's `/dev` and `/proc`, as if they were bind-mounted into the
root (`linux/sys.rs`, `system_path`); native chroots are not affected.

`GET_VOID_MIRROR` overrides `https://repo-default.voidlinux.org`. `/tmp` is
a tmpfs in the kernel heap, so a root there holds a few small packages and
is gone at reboot.

## Turning it on

| Where | What |
|-------|------|
| `Cargo.toml` (root) | feature `linux_compat` (not in `default` or `core`) |
| `kernel/Cargo.toml` | feature `linux-compat`, enabled by the root one |
| `linux-compat/build.sh` | builds the launcher and the Linux test binary for each arch (run automatically by `build.rs` when the feature is on) |
| `src/main.rs` | builds the aarch64 / riscv64 kernels with `linux-compat` when the host binary has `linux_compat` |

Without the feature the kernel has no Linux code, `SYS_LINUX_NEXT_EXEC`
(51) is an unknown syscall, and the image has no `/bin/etc/linux` or
`/bin/linux/`.

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
| `kernel/src/linux/mod.rs` | per-task state (personality, pending flag, thread pointer), the hooks the core calls, the auxv for new Linux images |
| `kernel/src/linux/x86_64.rs` | x86_64 syscall numbers, `struct stat`, signal frame, FXSAVE state, `arch_prctl` / FS base |
| `kernel/src/linux/aarch64.rs` | signal frame (`fpsimd_context`), `tpidr_el0` |
| `kernel/src/linux/riscv64.rs` | signal frame, the FPU enable for Linux tasks |
| `kernel/src/linux/generic.rs` | the `asm-generic` syscall numbers and `struct stat` aarch64 and riscv64 share |
| `kernel/src/linux/sys.rs` | the handlers: decode Linux arguments, call the native implementation, return `-errno` |
| `kernel/src/linux/signal.rs` | `rt_sigaction` & co., handler delivery, `rt_sigreturn`, the sigreturn trampoline page |
| `kernel/src/linux/abi.rs` | errno values, signal-number and sigset translation, `dirent64` layout |
| `kernel/src/linux/files.rs` | paths of the fds a Linux process opened (`fstat`, `getdents64`, `fchdir`, `*at`) |
| `linux-compat/launcher.c` | the `linux` command |
| `linux-compat/tests/linux-smoke.c` | Linux-side boot smoke (musl, static) |
| `linux-compat/tests/linux-dyn.c`, `libsmoke*.c` | dynamically linked smoke and its shared objects |
| `linux-compat/get-void.c` | the Void package fetcher (with zstd's decoder, fetched by `build.sh`) |

Hooks in the core, each behind `#[cfg(feature = "linux-compat")]`:

- `syscall_dispatch`: a Linux-personality task's syscalls go to
  `linux::dispatch` (then the usual signal handling at syscall exit);
- spawn / fork / exec (`task/lifecycle.rs`): reset, copy, or apply the
  personality;
- context switch (`task/sched.rs`): save and restore a Linux task's thread
  pointer (FP/SIMD registers are switched for every user task by the core,
  `task/fpu.rs`);
- signal delivery (`signal.rs`): a caught signal of a Linux task gets a
  Linux `rt_sigframe` instead of the native frame;
- riscv64 user entry (`user/mod.rs`, `user_sstatus`): Linux tasks run with
  the FPU on (`sstatus.FS`); native programs are soft-float;
- exec (`user/syscall.rs`): extra auxv entries (`AT_PHDR`, `AT_PHNUM`,
  `AT_ENTRY`, `AT_BASE`, ...) that musl's startup needs, and the dynamic
  linker of a dynamically linked program (see below).

The core refactors this needed are feature-independent: `exec_path`,
`open_path` and `chdir_path` take a kernel string (the native syscalls copy
the user string and call them), and `build_argv_stack` takes auxv entries.

## Supported syscalls

Numbers differ per arch (x86_64 has its own table; aarch64 and riscv64 share
the `asm-generic` one, which has only the `*at`, `clone`, `ppoll` and `dup3`
forms of the legacy calls). The set:

Files: `read`, `write`, `readv`, `writev`, `open`, `openat`, `close`, `stat`,
`lstat`, `fstat`, `newfstatat`, `lseek`, `getdents64`, `ioctl`, `access`,
`faccessat`, `pipe`, `pipe2`, `dup`, `dup2`, `dup3`, `fcntl` (dup; flags are
no-ops), `getcwd`, `chdir`, `fchdir`, `mkdir(at)`, `rmdir`, `unlink(at)`,
`rename(at/at2)`, `symlink(at)`, `readlink(at)`, `poll`, `umask`.

Memory: `brk`, `mmap` (anonymous, and private file mappings), `munmap`,
`mprotect`, `madvise` (no-op). Files also: `pread64`.

Processes: `fork`, `vfork` (as fork), `clone` (fork form only), `execve`,
`exit`, `exit_group`, `wait4`, `kill`, `tkill`, `tgkill`, `getpid`, `gettid`,
`getppid`, `getpgid`, `setpgid`, `getpgrp`, `getsid`, `setsid`, `uname`,
`arch_prctl`, `set_tid_address`, `set_robust_list`, `prlimit64`, `getrlimit`,
`get/set uid/gid` (everything is root), `sched_yield`.

Signals: `rt_sigaction` (handlers with `SA_SIGINFO`, `SA_RESTART`,
`SA_NODEFER`, `SA_RESETHAND`, `sa_mask`), `rt_sigreturn`, `rt_sigprocmask`,
`rt_sigpending`, `rt_sigsuspend`, `rt_sigtimedwait`, `sigaltstack` (no
alternate stacks). Signal numbers and masks are translated between Linux and
the native (newlib) numbering, also in `wait4` statuses.

Time and misc: `clock_gettime`, `gettimeofday`, `time`, `nanosleep`,
`clock_nanosleep`, `getrandom`; `poll` (x86_64) and `ppoll`.

Anything else returns `ENOSYS`.

## Dynamic linking

A dynamically linked program names its interpreter in `PT_INTERP` (musl:
`/lib/ld-musl-<arch>.so.1`, which is `libc.so` itself). For a Linux exec of
such a program the kernel:

1. reads the interpreter (`linux::exec_interp`) before replacing the current
   image, so a missing one fails the exec cleanly;
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

- file-backed `MAP_PRIVATE` mappings (the pages are a copy of the file,
  filled at map time; `MAP_SHARED` file mappings are refused);
- `MAP_FIXED` replaces whatever is mapped in its range;
- `munmap` of any range in the `mmap` window (holes included) and
  `mprotect` of part of a mapping split the mapping;
- free address space is reused (first fit) instead of only growing;
- a larger window (128 MiB on x86_64, 64 MiB on aarch64 / riscv64) and 64
  mappings per process; aarch64 user address spaces may span 128 MiB
  (previously 8 MiB).

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

## Limits

- PIE musl binaries only (static-PIE or dynamically linked), no glibc ones
  (not tried: glibc needs more syscalls), and no non-PIE `ET_EXEC` images:
  the loader places every image in the fixed
  per-arch user window (non-PIE binaries are linked at 0x400000 / 0x10000,
  which needs a redesign of the user address-space layout; so e.g. Alpine's
  prebuilt `busybox.static` does not load).
- Signals act at syscall exit, as for native programs: a task looping in
  user mode is not interrupted until its next syscall. No alternate signal
  stacks, no real-time signal queueing.
- No threads (`clone` with `CLONE_VM`), no shared file mappings
  (`MAP_SHARED`), no sockets, no `O_CLOEXEC` / `O_NONBLOCK` semantics.
- The native limits apply:
  - exec: up to 1024 arguments and 1024 environment strings, at most
    128 KiB together; a program file of at most 16 MiB from a writable
    filesystem (the initramfs has no limit) whose loaded image spans at
    most 1152 pages (4.5 MiB). Shared objects are mapped with `mmap` and
    do not count;
  - a per-process `mmap` window of 128 MiB (x86_64) / 64 MiB (aarch64,
    riscv64) with at most 64 mappings; adjacent mappings with the same
    protection are merged (musl's malloc makes hundreds of small
    neighbouring ones: jq peaks at 188);
  - a 16 MiB `brk` heap;
  - 64 fds per process, 64 tasks in total;
  - `/tmp` (tmpfs) files of at most 16 MiB each, all of them in the
    64 MiB kernel heap.

## Testing

`myos --ci` (and `MYOS_CI_MINI=1`) runs `linux /bin/linux/linux-smoke` and
`linux /bin/linux/linux-dyn` on every arch when the host binary was built
with `--features linux_compat`, and expects `LINUX-SMOKE OK` (files,
directories, mmap, fork/execve/wait4, pipes, Linux signal numbers, handlers,
masks, `sigwait`, `EINTR` and `SA_RESTART`) and `LINUX-DYN OK` (a call,
shared data, a relocated function pointer and a thread-local in
`libsmoke.so`, `printf` from `libc.so`, `dlopen`/`dlsym` of `libsmoke2.so`). The test is plain musl C, so it can also be run on a Linux
host for reference. On x86_64 and aarch64 it then runs
`get-void jq && linux --root /tmp/void jq -nr '"VOID-JQ \(1+2+3)"'` and
expects `VOID-JQ 6` (this needs the Void mirror to be reachable).

The default CI image does not include the layer. The CI build job
type-checks the three kernels with the feature
(`scripts/ci-build-pull-and-kernels.sh`), and the `linux-compat` jobs
(`.github/workflows/ci-runtime.yml`) build the layer's pieces and the
kernels with it on top of the CI ports, then boot bios, aarch64 and riscv64
in mini mode.

`linux-compat/build.sh` builds musl (static and shared) with clang for each
target. On aarch64/riscv64 musl's `long double` is 128-bit and needs
compiler-rt's quad-float builtins: the script fetches those sources (pinned
LLVM tag) and links them into `libc.so`. The static smoke test is not linked
against them, so it (and the launcher) avoid `printf`.
