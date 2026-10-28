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
| `toolchain/libmyos/sigtramp.c` | `__myos_sigtramp` (per arch) and the kernel calls (`__myos_sigaction_k`, `__myos_sigprocmask_k`, ...), shared by both libcs (`docs/mlibc.md`) |
| `toolchain/newlib/libgloss/myos/signal.c`, `toolchain/mlibc/sysdeps/myos/generic/signal.cpp` | each libc's `sigaction`, `signal`, `sigpending`, `sigsuspend`, `sigwait`, `pthread_sigmask` over them, and its handler dispatch |
| `toolchain/newlib/patch.sh` (`patch_signal_h`) | newlib `<sys/signal.h>`: `sa_sigaction`, `SA_RESTART`, `SA_NODEFER`, `SA_RESETHAND`, `SA_SIGINFO`, `SA_NOCLDWAIT`, `si_addr` and the fault `si_code`s |
| `kernel/src/arch/*/interrupts.rs` | the faults a program raises, as signals, and the signals acting when an interrupt preempts user mode (x86: the trap stubs that save its registers) |
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

Signals act on the way out of a syscall (`signal::on_syscall_exit`), and
when an interrupt (the timer, a reschedule IPI) preempts the task in user
mode (`signal::on_user_interrupt`, from the interrupt's trap frame: on x86
the timer and the IPI from ring 3 enter through stubs that save the user
registers, as the faults do). A process looping in user mode without
syscalls is thus ended by a signal whose action is to terminate (`SIGKILL`,
or `^C`'s `SIGINT` left at its default) and runs a caught one's handler,
which returns to the interrupted instruction with its registers. The
sender IPIs the CPU the task runs on (`task::signal_send` →
`sched::kick_running_locked`), so a signal acts at once rather than at the
task's next tick; a task blocked in a syscall is woken, and the signal acts
at that syscall's exit.

For a caught signal the kernel:

1. blocks `sa_mask` plus the signal itself (unless `SA_NODEFER`), and resets
   the handler for `SA_RESETHAND`;
2. writes a frame below the interrupted stack pointer (past the 128-byte
   x86-64 red zone): magic, signal number, handler, the mask to restore,
   and the interrupted PC, SP, syscall result and syscall-number register,
   plus a `siginfo_t` and what the trampoline's own syscall cannot keep
   (x86's `rflags`, `rcx` and `r11`);
3. resumes user mode at the trampoline registered with `sigaction`, with
   the stack pointer at the frame and every other register untouched.

`__myos_sigtramp` saves all integer and FP/SIMD registers (x86-64: FXSAVE
area; aarch64: `q0`–`q31`, FPCR/FPSR, NZCV; riscv64: integer registers, FP
ones on hard-float builds), calls the handler as `handler(sig, &siginfo,
NULL)`, restores everything and calls `SYS_SIGRETURN` with the stack pointer
back at the frame. The kernel restores the mask, PC, SP, the result
register and the syscall-number register (`x8` / `a7`), so the interrupted
code sees its syscall return normally. On x86 it puts back `rflags`, `rcx`
and `r11` too; when those are not what `sysret` would load (a return to a
faulting instruction, or into a handler), `syscall_entry` returns with
`iretq` instead.

## Faults

A synchronous fault in user mode raises its signal at once, from the trap's
registers (`signal::on_fault`), without waiting for a syscall:

| Fault | Signal | `si_code` | `si_addr` |
|-------|--------|-----------|-----------|
| an undefined instruction (x86 `#UD`, aarch64 EC 0, riscv64 cause 2) | `SIGILL` | `ILL_ILLOPN` (x86), `ILL_ILLOPC` | the instruction |
| a breakpoint (`int3`, `brk`, `ebreak`), x86's single step | `SIGTRAP` | `TRAP_BRKPT`, `TRAP_TRACE` | the instruction |
| an access a page or a mapping refuses (`PROT_NONE`, a store to a read-only page) | `SIGSEGV` | `SEGV_ACCERR` | the address |
| an access where nothing is mapped | `SIGSEGV` | `SEGV_MAPERR` | the address |
| x86 `#GP` (a privileged instruction, a non-canonical address), `into`, `bound` | `SIGSEGV` | `SI_KERNEL` | 0 |
| a misaligned access, PC or SP (x86 `#AC`, aarch64, riscv64) | `SIGBUS` | `BUS_ADRALN` | the address |
| x86 divide error | `SIGFPE` | `FPE_INTDIV` | the instruction |
| an FP exception that traps (x87, SSE, aarch64) | `SIGFPE` | `FPE_FLT*` from the status flags (`FPE_FLTUNK`, 14, when they say nothing: aarch64 without TFV; the libcs' headers do not name it) | the instruction |

A handler starts with x86's TF (the trap flag) and DF clear, as on Linux:
a program that single-steps itself (TF through `popf` or a signal frame)
gets a `SIGTRAP` after each of its own instructions, not in the handler,
which would fault again with `SIGTRAP` blocked; the frame keeps the flags,
and the handler's return steps on.

The codes are POSIX's, at Linux's values (newlib's `SI_USER` is 1 like
`SEGV_MAPERR`: tell them apart by the signal). A handler runs as for a
signal sent, and its return resumes the faulting instruction, which runs
again: the handler fixes the cause (an `mprotect`, as a garbage collector
or a JIT does) or leaves with `siglongjmp` (a CPU-feature probe). A page
not touched yet is paged in first and is no fault. When the program does
not catch the signal, or blocks it (Linux kills then too), the task ends by
the signal, after a `[ WARN ] user fault` line naming the fault and where
it happened (`docs/linux-compat.md`, Debugging). The Linux layer gets the
same faults in its own `rt_sigframe`, with `si_code` and `si_addr` (and
the address in the `sigcontext`'s `fault_address`, `cr2` on x86).

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
| 37 | `sigprocmask(how, set, oset)` | 32-bit masks |
| 45 | `sigreturn()` | frame at the user stack pointer |
| 46 | `waitpid(status, options, pid)` | POSIX `int` status (`WIFSIGNALED`); `pid > 0` that child, else any (`wait`); options bit 0 `WNOHANG`, bit 1 reports an exited child without reaping it (Linux `waitid` with `WNOWAIT`) |
| 47 | `sigpending()` | returns the pending set |
| 48 | `sigsuspend(mask)` | always `EINTR` |
| 49 | `sigwait(set)` | returns the signal taken |
| 50 | `sigaction(sig, act, oact)` | 4-word structs: `{handler, flags, mask, trampoline}`; without a trampoline only `SIG_DFL`/`SIG_IGN` |
| 88 | `itimer(which, new, old)` | the process's `ITIMER_REAL`: `SIGALRM` when it is due, then every interval (`setitimer`, `getitimer`, `alarm`) |

## Not yet

- `sigaltstack` / `SA_ONSTACK`, `sigqueue` / real-time signals, `ppoll`.
- `ITIMER_VIRTUAL` and `ITIMER_PROF` (`ENOSYS`): they need CPU time
  accounting.
- Stop/continue and job control.
