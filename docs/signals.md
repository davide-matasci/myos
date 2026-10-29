# Signals

myos delivers POSIX signals to user handlers in the kernel, as other Unix
kernels do. `sigaction` and `signal` handlers run, and `sigprocmask`
blocks. `kill` terminates a process with a `WIFSIGNALED` wait status.
Blocking syscalls return `EINTR`, or restart with `SA_RESTART`. Read this
page when you work on a libc's signal code, the kernel's delivery path or
a port that relies on signals.

## Pieces

| Where | What |
|-------|------|
| `kernel/src/signal.rs` | numbers, default actions, delivery, `sigreturn`, the signal syscalls |
| `kernel/src/task/signals.rs` | the per-thread pending and blocked bits, the per-process dispositions |
| `kernel/src/arch/*/interrupts.rs` | faults as signals, and signals acting when an interrupt preempts user mode |
| `toolchain/libmyos/sigtramp.c` | the trampoline `__myos_sigtramp` and the kernel calls, shared by both libcs (`docs/mlibc.md`) |
| `toolchain/newlib/libgloss/myos/signal.c`, `toolchain/mlibc/sysdeps/myos/generic/signal.cpp` | each libc's `sigaction` family and handler dispatch |

Signal numbers are newlib's, in the BSD layout (`SIGCHLD = 20`,
`SIGUSR1 = 30`); `toolchain/newlib/patch.sh` adds what newlib's
`<sys/signal.h>` lacks. `raise` is `kill(getpid(), sig)`. `kill` takes a
task (`pid > 0`), the caller's group (`0`) or group `-pid`; signal 0
only checks that the target exists. The masks are 32 bits.

## Default actions

The default action is to terminate, except for `SIGCHLD`, `SIGURG`,
`SIGWINCH` and `SIGCONT`, which are ignored. There is no job control yet,
so `SIGSTOP`, `SIGTSTP`, `SIGTTIN` and `SIGTTOU` are ignored too.
`SIGKILL` and `SIGSTOP` cannot be caught, ignored or blocked.

An ignored signal is dropped when sent, unless it is blocked: a blocked
signal always stays pending, because the disposition may change before
it is unblocked (as on Linux).

`fork` copies the handlers and the blocked mask. `exec` resets caught
signals to `SIG_DFL` and keeps ignored ones, the blocked mask and the
pending signals.

With threads (`docs/threads.md`), the dispositions belong to the process,
the blocked mask and pending set to each thread. A signal sent to a pid
goes to the leader thread, one sent to a tid to that thread. A signal
that terminates ends the whole process.

A process that ignores `SIGCHLD`, or sets `SA_NOCLDWAIT` on it, gets no
zombies: the kernel reaps its exiting children, and `wait` returns
`ECHILD` once none is left. `SA_NOCLDWAIT` is `0x20`, because Linux's
value 2 is newlib's `SA_SIGINFO`; the Linux layer translates.

## Delivery

Signals act on the way out of a syscall and when an interrupt (the timer,
a reschedule IPI) preempts the task in user mode. So a process looping
without syscalls is still reached. The sender IPIs the CPU the task runs
on, so the signal acts at once; a task blocked in a syscall is woken and
the signal acts at that syscall's exit.

For a caught signal the kernel writes a frame with the interrupted state
and a `siginfo_t` below the user stack pointer, and resumes user mode at
the trampoline the libc registered with `sigaction` (`__myos_sigtramp`).
The trampoline saves every register, calls `handler(sig, &siginfo, NULL)`
and returns through `SYS_SIGRETURN`, so the interrupted code sees its
syscall return normally.

## Faults

A synchronous fault in user mode raises its signal at once, from the
trap's registers (`signal::on_fault`):

| Fault | Signal | `si_code` | `si_addr` |
|-------|--------|-----------|-----------|
| an undefined instruction | `SIGILL` | `ILL_ILLOPN` (x86), `ILL_ILLOPC` | the instruction |
| a breakpoint (`int3`, `brk`, `ebreak`), x86's single step | `SIGTRAP` | `TRAP_BRKPT`, `TRAP_TRACE` | the instruction |
| an access a mapping refuses (`PROT_NONE`, a store to a read-only page) | `SIGSEGV` | `SEGV_ACCERR` | the address |
| an access where nothing is mapped | `SIGSEGV` | `SEGV_MAPERR` | the address |
| x86 `#GP` (a privileged instruction, a non-canonical address), `into`, `bound` | `SIGSEGV` | `SI_KERNEL` | 0 |
| a misaligned access, PC or SP | `SIGBUS` | `BUS_ADRALN` | the address |
| x86 divide error | `SIGFPE` | `FPE_INTDIV` | the instruction |
| a trapping FP exception (x87, SSE, aarch64) | `SIGFPE` | `FPE_FLT*` from the status flags, `FPE_FLTUNK` (14) when they say nothing | the instruction |

The codes are POSIX's, at Linux's values. newlib's `SI_USER` is 1, like
`SEGV_MAPERR`: tell them apart by the signal.

A handler's return resumes the faulting instruction, which runs again. So
the handler either fixes the cause (an `mprotect`, as a garbage collector
or a JIT does) or leaves with `siglongjmp` (a CPU-feature probe). When
the program does not catch the signal, or blocks it, the task ends by
the signal, after a `[ WARN ] user fault` line in the kernel log
(`docs/linux-compat.md`, Debugging). The Linux layer gets the same faults
in its own `rt_sigframe`.

## Process groups and sessions

Groups and sessions decide what `kill(0)`, `kill(-pgid)` and a terminal's
`^C` and hangup reach (`kernel/src/task/jobs.rs`).

- A process spawned by the kernel leads its own session and group. A
  forked child inherits both.
- `setpgid` moves the caller, or a child that has not exec'd yet
  (`EACCES` after), into a new group or one of its session. A session
  leader cannot change its group (`EPERM`).
- `setsid` starts a session and a group led by the caller, with no
  controlling terminal (`EPERM` for a group leader). A pty the new
  session claims becomes its terminal (`docs/tty.md`).
- Like a Linux pid, a task id is not handed out again while a group or
  session still goes by it.

## Timers

`setitimer(ITIMER_REAL)`, `getitimer` and `alarm` share one timer per
process, on the monotonic clock. It sends `SIGALRM` when due and re-arms
by its interval; expiries missed under load are one signal, as on Linux.
A fork starts without a timer; exec keeps it.

## Interrupted syscalls

Blocking waits (console, pty and pipe reads, full-pipe writes, FIFO
opens, `wait`, `poll` and `select`) stop when a signal would terminate
the task or run a handler. The syscall then returns `EINTR` if the
handler lacks `SA_RESTART`, and restarts if it has it or if nothing acts
on the signal any more.

- `signal()` installs BSD-style handlers (`SA_RESTART`).
- `pselect` sets its mask around `select`, not atomically: a signal it
  unblocks just before the wait runs its handler without ending the wait.

## Limits

Not implemented yet:

- `sigaltstack` / `SA_ONSTACK`, `sigqueue` / real-time signals, `ppoll`.
- `ITIMER_VIRTUAL` and `ITIMER_PROF` (`ENOSYS`): the kernel does not split
  a task's user and kernel time (issue #291).
- Stop/continue and job control.
