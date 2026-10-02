# Signals

myos delivers POSIX signals to user handlers in the kernel, like other Unix
kernels: `sigaction`/`signal` handlers run, `sigprocmask` blocks, `kill`
terminates with a `WIFSIGNALED` wait status, and blocking syscalls return
`EINTR` (or restart, with `SA_RESTART`).

## Pieces

| Where | What |
|-------|------|
| `kernel/src/signal.rs` | policy: signal numbers, default actions, delivery, `sigreturn`, `sigaction`/`sigprocmask`/`sigsuspend`/`sigwait` |
| `kernel/src/task/signals.rs` | per-thread pending / blocked bits; per-process ignored bits, caught-handler table and libc trampoline address |
| `toolchain/newlib/libgloss/myos/signal.c` | `__myos_sigtramp` (per arch), `sigaction`, `signal`, `sigpending`, `sigsuspend`, `sigwait`, `pthread_sigmask` |
| `toolchain/newlib/patch.sh` (`patch_signal_h`) | newlib `<sys/signal.h>`: `sa_sigaction`, `SA_RESTART`, `SA_NODEFER`, `SA_RESETHAND`, `SA_SIGINFO` |
| `toolchain/newlib/build.sh` | `-DSIGNAL_PROVIDED`: newlib's userspace `signal()`/`raise()` emulation is off; `raise` is `kill(getpid(), sig)` |

Signal numbers are newlib's (BSD layout: `SIGCHLD = 20`, `SIGUSR1 = 30`),
which libgloss, the ports and `rustix_compat` are built against.

## Default actions

Terminate, except `SIGCHLD`, `SIGURG`, `SIGWINCH` and `SIGCONT` (ignored).
There is no job control yet, so `SIGSTOP`/`SIGTSTP`/`SIGTTIN`/`SIGTTOU` are
ignored too. `SIGKILL` and `SIGSTOP` cannot be caught, ignored or blocked.

A signal whose disposition is "ignore" is dropped when sent, unless it is
blocked: the disposition may change before it is unblocked, so a blocked
signal always stays pending (as on Linux).

`fork` copies handlers and the blocked mask; `exec` resets caught signals to
`SIG_DFL` and keeps ignored ones, the blocked mask and pending signals.

With threads (`docs/threads.md`), dispositions belong to the process and
the blocked mask and pending set to each thread. A signal sent to a pid goes
to the process's leader thread, one sent to a tid to that thread; a signal
that terminates ends the whole process.

## Delivery

Signals act on the way out of a syscall (`signal::on_syscall_exit`), as the
default actions always did. A process looping in user mode without syscalls
is not interrupted until its next syscall, except by `SIGKILL`, which also
acts when an interrupt preempts it in user mode
(`signal::on_user_preempted`).

For a caught signal the kernel:

1. blocks `sa_mask` plus the signal itself (unless `SA_NODEFER`), and resets
   the handler for `SA_RESETHAND`;
2. writes a frame below the interrupted stack pointer (past the 128-byte
   x86-64 red zone): magic, signal number, handler, the mask to restore,
   and the interrupted PC, SP, syscall result and syscall-number register,
   plus a `siginfo_t`;
3. resumes user mode at the trampoline registered with `sigaction`, with
   the stack pointer at the frame and every other register untouched.

`__myos_sigtramp` saves all integer and FP/SIMD registers (x86-64: FXSAVE
area; aarch64: `q0`–`q31`, FPCR/FPSR, NZCV; riscv64: integer registers, FP
ones on hard-float builds), calls the handler as `handler(sig, &siginfo,
NULL)`, restores everything and calls `SYS_SIGRETURN` with the stack pointer
back at the frame. The kernel restores the mask, PC, SP, the result
register and the syscall-number register (`x8` / `a7`), so the interrupted
code sees its syscall return normally.

## Interrupted syscalls

Blocking waits (console/pty/pipe reads, full-pipe writes, FIFO opens,
`wait`) stop when a signal would terminate the task or run a handler
(`signal::interrupt_wait`). On the way out the syscall then

- returns `EINTR` (`-4`, mapped to `errno` by libgloss) if the handler does
  not have `SA_RESTART`;
- restarts if it does: the saved PC is rewound to the syscall instruction
  and the number / first-argument register restored;
- restarts transparently if, by then, nothing acts on the signal.

`select`/`poll` wait in userspace loops that enter the kernel every
iteration; they return `EINTR` when a handler ran (libgloss counts them).
`sigsuspend` waits with a temporary mask and the handler returns to the
previous one. `signal()` installs BSD-style handlers (`SA_RESTART`).

## Syscalls

| # | Name | Notes |
|--:|------|-------|
| 34 | `kill(pid, sig)` | `pid > 0` task, `0` own group, `< 0` group `-pid` |
| 35 | `sigaction(sig, act, oact)` | 3-word struct, `SIG_DFL`/`SIG_IGN` only (binaries from before handlers) |
| 37 | `sigprocmask(how, set, oset)` | 32-bit masks |
| 45 | `sigreturn()` | frame at the user stack pointer |
| 46 | `waitpid(status, options, pid)` | POSIX `int` status (`WIFSIGNALED`); `SYS_WAIT` (7) keeps the exit-code byte |
| 47 | `sigpending()` | returns the pending set |
| 48 | `sigsuspend(mask)` | always `EINTR` |
| 49 | `sigwait(set)` | returns the signal taken |
| 50 | `sigaction2(sig, act, oact)` | 4-word struct: `{handler, flags, mask, trampoline}` |

Syscalls 39, 41 and 42 (`SIGCHLD_TAKE`, `SIGCHLD_PENDING`, `PIPE_PEER`) are
the old libgloss SIGCHLD polling, kept for binaries built before this.

## Not yet

- Asynchronous delivery of signals other than `SIGKILL` to a process that
  makes no syscalls (on the interrupt-return path).
- `sigaltstack` / `SA_ONSTACK`, `sigqueue` / real-time signals, `alarm` /
  timers, `ppoll`.
- Stop/continue and job control.
