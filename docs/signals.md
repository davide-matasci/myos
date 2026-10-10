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
| `toolchain/newlib/patch.sh` (`patch_signal_h`) | newlib `<sys/signal.h>`: `sa_sigaction`, `SA_RESTART`, `SA_NODEFER`, `SA_RESETHAND`, `SA_SIGINFO`, `SA_NOCLDWAIT` |
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

## Children without zombies

A process that ignores `SIGCHLD`, or sets `SA_NOCLDWAIT` on it (`0x20`:
Linux's 2 is newlib's `SA_SIGINFO`; the Linux layer translates), gets no
zombies: an exiting child is reaped by the kernel like an orphan
(`die` in `kernel/src/task/lifecycle.rs`), and `wait` finds one child fewer,
`ECHILD` once none is left (a `wait` in progress returns `ECHILD` when the
last child exits). `SIGCHLD` itself is still sent to a handler set with
`SA_NOCLDWAIT`.

With threads (`docs/threads.md`), dispositions belong to the process and
the blocked mask and pending set to each thread. A signal sent to a pid goes
to the process's leader thread, one sent to a tid to that thread; a signal
that terminates ends the whole process.

## Delivery

Signals act on the way out of a syscall (`signal::on_syscall_exit`), as the
default actions always did. A process looping in user mode without syscalls
is also ended when an interrupt preempts it in user mode
(`signal::on_user_preempted`) by a pending signal whose action is to
terminate (`SIGKILL`, or `^C`'s `SIGINT` left at its default); a caught
signal waits for its next syscall.

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

## Process groups and sessions

What `kill(0)`, `kill(-pgid)` and a terminal's `^C` and hangup reach
([`kernel/src/task/jobs.rs`](../kernel/src/task/jobs.rs)). A process
spawned by the kernel leads its own session and group; a forked child
inherits both. `setpgid` moves the caller, or a child that has not exec'd
yet (`EACCES` after), into a new group or one of its session; a session
leader cannot change its group (`EPERM`). `setsid` starts a session and a
group led by the caller, with no controlling terminal, and fails (`EPERM`)
for a process that already leads a group. A pty the new session claims
becomes its terminal (`docs/tty.md`). Like a Linux pid, a task id is not
handed out again while a group or session still goes by it.

## Timers

`setitimer(ITIMER_REAL)`, `getitimer` and `alarm` are one kernel timer per
process, on the monotonic clock, kept in the leader's task slot: the timer
interrupt sends `SIGALRM` once it is due and re-arms it by its interval
(expiries missed under load are one signal, as on Linux). A fork starts
without one, exec keeps it. Like any signal it acts when the process next
enters or leaves the kernel ("Delivery"): it ends a blocking wait (`EINTR`)
at once.

## Interrupted syscalls

Blocking waits (console/pty/pipe reads, full-pipe writes, FIFO opens,
`wait`) stop when a signal would terminate the task or run a handler
(`signal::interrupt_wait`). On the way out the syscall then

- returns `EINTR` (`-4`, mapped to `errno` by libgloss) if the handler does
  not have `SA_RESTART`;
- restarts if it does: the saved PC is rewound to the syscall instruction
  and the number / first-argument register restored;
- restarts transparently if, by then, nothing acts on the signal.

`poll` (and `select`, built on it) waits in the kernel (`SYS_POLL`) and
returns `EINTR` like any other blocking call. `pselect` sets its mask
around `select` rather than atomically: a signal it unblocks that arrives
just before the wait runs its handler without ending the wait.
`sigsuspend` waits with a temporary mask and the handler returns to the
previous one. `signal()` installs BSD-style handlers (`SA_RESTART`).

## Syscalls

| # | Name | Notes |
|--:|------|-------|
| 34 | `kill(pid, sig)` | `pid > 0` task, `0` own group, `< 0` group `-pid`; `sig` 0 sends nothing, only checks that the target exists (`ESRCH` for an exited one, zombies included) |
| 35 | `sigaction(sig, act, oact)` | 3-word struct, `SIG_DFL`/`SIG_IGN` only (binaries from before handlers) |
| 37 | `sigprocmask(how, set, oset)` | 32-bit masks |
| 45 | `sigreturn()` | frame at the user stack pointer |
| 46 | `waitpid(status, options, pid)` | POSIX `int` status (`WIFSIGNALED`); `SYS_WAIT` (7) keeps the exit-code byte |
| 47 | `sigpending()` | returns the pending set |
| 48 | `sigsuspend(mask)` | always `EINTR` |
| 49 | `sigwait(set)` | returns the signal taken |
| 50 | `sigaction2(sig, act, oact)` | 4-word struct: `{handler, flags, mask, trampoline}` |
| 88 | `itimer(which, new, old)` | the process's `ITIMER_REAL`: `SIGALRM` when it is due, then every interval (`setitimer`, `getitimer`, `alarm`) |

Syscalls 39, 41 and 42 (`SIGCHLD_TAKE`, `SIGCHLD_PENDING`, `PIPE_PEER`) are
the old libgloss SIGCHLD polling, kept for binaries built before this.

## Not yet

- Asynchronous delivery of signals other than `SIGKILL` to a process that
  makes no syscalls (on the interrupt-return path).
- `sigaltstack` / `SA_ONSTACK`, `sigqueue` / real-time signals, `ppoll`.
- `ITIMER_VIRTUAL` and `ITIMER_PROF` (`ENOSYS`): they need CPU time
  accounting.
- Stop/continue and job control.
