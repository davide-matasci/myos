# Linux syscall compatibility layer (optional)

> **Optional, off by default.** None of this is in a default build: the
> kernel code is behind a Cargo feature, and nothing about native myos
> programs changes when it is on. myos's own syscall ABI stays the primary
> interface; this layer is an add-on for running some unmodified Linux
> binaries.

With it, a static-PIE Linux binary linked against musl (x86_64, aarch64 or
riscv64) runs on myos when started through the `linux` launcher:

```sh
cargo run --features linux_compat          # build + boot the x86_64 image
cargo run --features linux_compat -- aarch64   # (or riscv64)
# in the guest:
linux /bin/linux/linux-smoke               # prints LINUX-SMOKE OK
```

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
| `kernel/src/linux/mod.rs` | per-task state (personality, pending flag, thread pointer, saved FP/SIMD registers), the hooks the core calls, the auxv for new Linux images |
| `kernel/src/linux/x86_64.rs` | x86_64 syscall numbers, `struct stat`, signal frame, FXSAVE state, `arch_prctl` / FS base |
| `kernel/src/linux/aarch64.rs` | signal frame (`fpsimd_context`), q0-q31/FPSR/FPCR, `tpidr_el0` |
| `kernel/src/linux/riscv64.rs` | signal frame, f0-f31/fcsr, the FPU enable for Linux tasks |
| `kernel/src/linux/generic.rs` | the `asm-generic` syscall numbers and `struct stat` aarch64 and riscv64 share |
| `kernel/src/linux/sys.rs` | the handlers: decode Linux arguments, call the native implementation, return `-errno` |
| `kernel/src/linux/signal.rs` | `rt_sigaction` & co., handler delivery, `rt_sigreturn`, the sigreturn trampoline page |
| `kernel/src/linux/abi.rs` | errno values, signal-number and sigset translation, `dirent64` layout |
| `kernel/src/linux/files.rs` | paths of the fds a Linux process opened (`fstat`, `getdents64`, `fchdir`, `*at`) |
| `linux-compat/launcher.c` | the `linux` command |
| `linux-compat/tests/linux-smoke.c` | Linux-side boot smoke (musl) |

Hooks in the core, each behind `#[cfg(feature = "linux-compat")]`:

- `syscall_dispatch`: a Linux-personality task's syscalls go to
  `linux::dispatch` (then the usual signal handling at syscall exit);
- spawn / fork / exec (`task/lifecycle.rs`): reset, copy, or apply the
  personality;
- context switch (`task/sched.rs`): save and restore a Linux task's thread
  pointer and FP/SIMD registers;
- signal delivery (`signal.rs`): a caught signal of a Linux task gets a
  Linux `rt_sigframe` instead of the native frame;
- riscv64 user entry (`user/mod.rs`, `user_sstatus`): Linux tasks run with
  the FPU on (`sstatus.FS`); native programs are soft-float;
- exec (`user/syscall.rs`): extra auxv entries (`AT_PHDR`, `AT_PHNUM`,
  `AT_ENTRY`, ...) that musl's static-PIE startup needs.

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

Memory: `brk`, `mmap` (anonymous only), `munmap`, `mprotect`, `madvise` (no-op).

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

- Static-PIE musl binaries only. No dynamic linking (`PT_INTERP`), and no
  non-PIE `ET_EXEC` images: the loader places every image in the fixed
  per-arch user window (non-PIE binaries are linked at 0x400000 / 0x10000,
  which needs a redesign of the user address-space layout; so e.g. Alpine's
  prebuilt `busybox.static` does not load).
- Signals act at syscall exit, as for native programs: a task looping in
  user mode is not interrupted until its next syscall. No alternate signal
  stacks, no real-time signal queueing.
- FP/SIMD registers are saved across context switches for Linux tasks only
  (native programs do not get this yet).
- No threads (`clone` with `CLONE_VM`), no file-backed `mmap`, no sockets,
  no `O_CLOEXEC` / `O_NONBLOCK` semantics.
- The native limits apply: 16 args / 32 environment strings of at most 128
  bytes at exec, a 1 MiB anonymous-`mmap` window, 32 fds.

## Testing

`myos --ci` (and `MYOS_CI_MINI=1`) runs `linux /bin/linux/linux-smoke` on
every arch when the host binary was built with `--features linux_compat` and
expects `LINUX-SMOKE OK` (files, directories, mmap, fork/execve/wait4,
pipes, Linux signal numbers, handlers, masks, `sigwait`, `EINTR` and
`SA_RESTART`). The test is plain musl C, so it can also be run on a Linux
host for reference. The default CI image does not include the layer; the
CI build job type-checks the three kernels with the feature
(`scripts/ci-build-pull-and-kernels.sh`) so it keeps compiling.

`linux-compat/build.sh` builds musl with clang for each target. musl's
`printf` needs quad-float compiler-runtime helpers on aarch64/riscv64 that
this build does not ship, so the smoke test (and the launcher) avoid it.
