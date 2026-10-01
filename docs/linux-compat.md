# Linux syscall compatibility layer (optional)

> **Optional, off by default.** None of this is in a default build: the
> kernel code is behind a Cargo feature, and nothing about native myos
> programs changes when it is on. myos's own syscall ABI stays the primary
> interface; this layer is an add-on for running some unmodified Linux
> binaries.

With it, a static-PIE x86_64 Linux binary linked against musl runs on myos
when started through the `linux` launcher:

```sh
cargo run --features linux_compat          # build + boot the x86_64 image
# in the guest:
linux /bin/linux/linux-smoke               # prints LINUX-SMOKE OK
```

## Turning it on

| Where | What |
|-------|------|
| `Cargo.toml` (root) | feature `linux_compat` (not in `default` or `core`) |
| `kernel/Cargo.toml` | feature `linux-compat`, enabled by the root one |
| `linux-compat/build.sh` | builds the launcher and the Linux test binary (run automatically by `build.rs` when the feature is on) |

Without the feature the kernel has no Linux code, `SYS_LINUX_NEXT_EXEC`
(51) is an unknown syscall, and the image has no `/bin/etc/linux` or
`/bin/linux/`. The feature builds only for x86_64; the aarch64 and riscv64
kernels are built without it (enabling it there is a compile error).

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
| `kernel/src/linux/x86_64.rs` | x86_64 syscall numbers → handlers; `arch_prctl` / FS base |
| `kernel/src/linux/sys.rs` | the handlers: decode Linux arguments, call the native implementation, return `-errno` |
| `kernel/src/linux/abi.rs` | errno values, signal-number and sigset translation, `struct stat` / `dirent64` layouts |
| `kernel/src/linux/files.rs` | paths of the fds a Linux process opened (`fstat`, `getdents64`, `fchdir`, `*at`) |
| `linux-compat/launcher.c` | the `linux` command |
| `linux-compat/tests/linux-smoke.c` | Linux-side boot smoke (musl) |

Hooks in the core, each behind `#[cfg(feature = "linux-compat")]`:

- `syscall_dispatch`: a Linux-personality task's syscalls go to
  `linux::dispatch` (then the usual signal handling at syscall exit);
- spawn / fork / exec (`task/lifecycle.rs`): reset, copy, or apply the
  personality;
- context switch (`task/sched.rs`): load the task's FS base (musl's thread
  pointer);
- exec (`user/syscall.rs`): extra auxv entries (`AT_PHDR`, `AT_PHNUM`,
  `AT_ENTRY`, ...) that musl's static-PIE startup needs.

The core refactors this needed are feature-independent: `exec_path`,
`open_path` and `chdir_path` take a kernel string (the native syscalls copy
the user string and call them), and `build_argv_stack` takes auxv entries.

## Supported syscalls (x86_64)

Files: `read`, `write`, `readv`, `writev`, `open`, `openat`, `close`, `stat`,
`lstat`, `fstat`, `newfstatat`, `lseek`, `getdents64`, `ioctl`, `access`,
`faccessat`, `pipe`, `pipe2`, `dup`, `dup2`, `dup3`, `fcntl` (dup; flags are
no-ops), `getcwd`, `chdir`, `fchdir`, `mkdir(at)`, `rmdir`, `unlink(at)`,
`rename(at/at2)`, `symlink(at)`, `readlink(at)`, `poll`, `umask`.

Memory: `brk`, `mmap` (anonymous only), `munmap`, `mprotect`, `madvise` (no-op).

Processes: `fork`, `vfork` (as fork), `clone` (fork form only), `execve`,
`exit`, `exit_group`, `wait4`, `kill`, `tkill`, `tgkill`, `getpid`, `gettid`,
`getppid`, `getpgid`, `setpgid`, `getpgrp`, `getsid`, `setsid`, `uname`,
`arch_prctl`, `set_tid_address`, `set_robust_list`, `prlimit64`, `getrlimit`,
`get/set uid/gid` (everything is root), `sched_yield`.

Signals: `rt_sigaction` (`SIG_DFL` / `SIG_IGN`), `rt_sigprocmask`,
`sigaltstack` (no-op). Signal numbers and masks are translated between Linux
and the native (newlib) numbering, also in `wait4` statuses.

Time and misc: `clock_gettime`, `gettimeofday`, `time`, `nanosleep`,
`clock_nanosleep`, `getrandom`.

Anything else returns `ENOSYS`.

## Limits

- x86_64 only; static-PIE musl binaries only. No dynamic linking
  (`PT_INTERP`), and no non-PIE `ET_EXEC` images: the loader places every
  image at the per-process user base (so e.g. Alpine's prebuilt
  `busybox.static`, linked at 0x400000, does not load).
- Signal handlers installed with `rt_sigaction` are accepted but not run
  yet: the signal keeps its default action. No `rt_sigreturn`.
- No threads (`clone` with `CLONE_VM`), no file-backed `mmap`, no sockets,
  no `O_CLOEXEC` / `O_NONBLOCK` semantics.
- The native limits apply: 16 args / 32 environment strings of at most 128
  bytes at exec, a 1 MiB anonymous-`mmap` window, 32 fds.

## Testing

`myos --ci` (and `MYOS_CI_MINI=1`) runs `linux /bin/linux/linux-smoke` on
x86_64 boots when the host binary was built with `--features linux_compat`
and expects `LINUX-SMOKE OK`. The default CI image does not include the
layer; the CI build job type-checks the kernel with the feature
(`scripts/ci-build-pull-and-kernels.sh`) so it keeps compiling.
