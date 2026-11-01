# Scheduling, SMP and waits

This page covers how myos runs tasks on several CPUs: how the CPUs come
up, how the scheduler picks and places tasks, how a task blocks and is
woken, and how timers and idle CPUs behave. Read it before changing
`kernel/src/task/sched.rs`, `kernel/src/smp.rs`, a wait in a syscall or
an arch's timer code. Threads, process lifetime and the TLB rules for an
address space loaded on several CPUs are in `docs/threads.md`.

## Watching it

`/proc/cpuinfo` reports per CPU `schedules`, `idle_halts` and `pulls`,
and overall `blocked_tasks`, `clock_hz` and `uptime_ms`. An idle shell
should show `idle_halts` climbing and `schedules` nearly flat.

A CPU that stops scheduling while not idle is reported on the screen
and the serial console (`kernel/src/task/lockup.rs`). Every CPU passes
through the scheduler at each tick unless it halts idle; one that has
not for 10 s is stuck in the kernel, its interrupts off or spinning on
the task table, and the tasks queued on it never run again (the console
input thread among them: the machine looks frozen, though CPU 0 still
blinks the cursor). CPU 0's tick looks once a second and says, once per
stall, which CPU and which task, and how long the stall lasted if the
CPU goes on:

```
[ WARN ] cpu 3 stuck for 10 s in the kernel: pid 41 wifi (slot 43)
[ WARN ] cpu 3 went on after 14 s
```

On x86_64 a line before it says where the CPU is. CPU 0 sends it an NMI,
which interrupts it even with its interrupts off. The NMI handler, on a
stack of its own, notes the address the CPU was at and, in the kernel,
the words on its stack that point into the kernel's code. The kernel
keeps no frame pointers, so these are likely its callers, not a sure
backtrace. `addr2line -e` on the build's kernel names the functions:

```
[ WARN ] cpu 3 at 0xffffffff8012a3f0 in the kernel, called from 0xffffffff80131c22 0xffffffff800f0a17
```

aarch64 and riscv64 have no such interrupt here, so their report names
the task alone.

It allocates nothing and waits on no lock, since the stuck CPU may hold
any: the task is named only when the task table is free, and a line the
console or its screen is busy for (the console module's non-blocking
`status_line_try`, `docs/modules.md`) waits for the next second. CPU 0 itself is not
watched: its tick is the watcher, so a freeze of every CPU, CPU 0's tick
included, says nothing. The boot tests fail on the report
(`docs/testing.md`).

QEMU runs `-smp 4` on x86_64 and aarch64. riscv64 stays at `-smp 2`,
because Limine panics with `missing struct riscv_hart for BSP` at 4; the
packed `boot/virt.dtb` is dumped with `-smp 2` too, so that OpenSBI's
BSP, hartid 1, still has a hart node.

The tests carry the tag `sched`: `sched_smoke` (`user/c`), `pthread`,
`std_thread`, and `idle_tickless`, `idle_cpu0` and `console_irqs` in
`user/tests/kernel.sh`.

## CPUs per architecture

All three arches start their APs through Limine's `MpRequest`, and the
APs stay online into userspace. The per-arch entry code is in
`kernel/src/smp.rs` and `kernel/src/arch/<arch>`.

| Arch | CPU id | Timer | IPI |
|------|--------|-------|-----|
| x86_64 | TSC_AUX / APIC id | LAPIC timer (x2APIC when present; TCG has none, so CI runs the xAPIC path) | LAPIC ICR |
| aarch64 | `TPIDR_EL1` / `MPIDR_EL1` | generic timer PPI | GIC SGI |
| riscv64 | `tp` / Limine `hartid` | `stimecmp` | SBI IPI |

An AP keeps its IRQs masked until it runs on its own 64 KiB idle stack:
Limine's AP stacks are too small for nested timer and IPI frames. Each
CPU has its own kernel entry state (x86_64's GDT and TSS; on aarch64 and
riscv64, the syscall frame that fork and exec use, saved and restored by
`schedule`, so a syscall that blocks and resumes on another CPU finds its
own frame).

## Run queues and home CPUs

Ready tasks that are on no CPU sit in per-CPU run queues (`sched::RUNQ`,
a bit per task slot) for the tasks homed on that CPU, and in one floating
queue (`FLOAT`) for the kernel's threads, which may run anywhere. Picking
is a few bit operations, round-robin after the current slot; idle tasks
are never queued.

### Where tasks run

- Kernel tasks float; AP idle threads are pinned.
- Each new user task gets a home CPU round-robin (`user_affinity` in
  `kernel/src/task/lifecycle.rs`). On x86_64 and aarch64 that skips the
  BSP; on riscv64 it uses both harts. Threads get homes the same way.
- `fork` gives the child its parent's home, which keeps a sequential
  fork+exec+wait on one CPU and init's children (netd) with init. But a
  parent with a controlling terminal that already has an active child
  (`make -j`, a pipeline) gives the new child a fresh home
  (`parent_has_active_child`).
- `exec` keeps the home: there is no re-homing after exec.
- A user task changes CPU only while it is Ready and off every CPU (a
  pull, below), never while it runs, and always has a home.

### Idle-pull balancing

`sched::pull` balances the load. A CPU with nothing in its queues takes a
user task from the queue of a busy (non-idle) CPU and becomes its home. A
task queued for a busy home, or a floating task, kicks an idle CPU that
may run it (`kick_for`), so the pull happens at once. Kernel threads are
never pulled, and user tasks are never pulled onto the BSP where
`user_affinity` keeps them off it.

A user task runs on one CPU at a time and is loaded only there
(`LOADED_ASPACE`). The CPU it left switched to another root before
queueing it, which flushed its TLB, so the thread TLB rules cover a
pulled task too.

### Switching

`schedule` switches the address space before it publishes the old task
as Ready, without the lock held. A task leaving a CPU stays Running until
`finish_switch`, even if a wake made it Ready meanwhile: left Ready, a
peer could resume it from a stale switch frame. A wake that lands
mid-switch is deferred to `finish_switch`.

A user task's FP/SIMD registers are saved when it leaves a CPU and
restored when it next runs (`task/fpu.rs`). On x86 a CPU with XSAVE and
AVX gets AVX state turned on at boot (CR4.OSXSAVE, XCR0 = x87 | SSE |
AVX, `arch/x86/fpu.rs`), as a program that finds AVX in CPUID expects,
and the image is XSAVE's (with ymm0-15's upper halves); without, FXSAVE's.
A new image (spawn, exec) starts from the registers' initial state (x87's
control word 0x37f, MXCSR 0x1f80, every register zero), not from what the
last task left on the CPU. `heap`'s `avx` stage checks the AVX state on
the CI machines, whose CPU model has it.

## Idle CPUs

`kernel_main` (task 0) is the BSP's idle task, and `ap_idle_body` the
APs'. Each pass (`idle_step`) marks the CPU idle, picks from its queue,
then the floating one, else tries a pull, and halts when that found
nothing, unless a reschedule was flagged meanwhile. A task that blocks
with nothing else runnable halts the same way, from `block_until`.

`wake` queues the woken task for its home and sends a **targeted**
reschedule IPI only when that CPU is halted.

## TLB shootdown

`smp::tlb_shootdown` invalidates locally, then sends an IPI to the other
online CPUs and waits briefly for their acks.

- An idle CPU runs no user code, so it is not interrupted: it is flagged
  (`TLB_STALE`) and flushes when it next resumes a task.
- A CPU waiting with IRQs off still serves a shootdown: `schedule` calls
  `tlb_service`.
- No IPI is sent when no other CPU has the address space loaded
  (`task::aspace_loaded_elsewhere`).
- It runs with interrupts off, whatever its caller's. Its lock is a spin
  lock that other CPUs wait on with theirs off. A holder preempted with
  the lock held, as the reclaim of an exiting process could be, never ran
  again while they spun, each up to its spin cap.

Do not paper over a slow shootdown path with a longer QEMU timeout.

## Blocking waits

A task gives up its CPU when it yields (`task::yield_now`), when the
timer preempts it (in user mode too), or when it blocks. Every kernel
wait (`wait`, tty, pipe, FIFO and pty I/O, `sigsuspend`, `nanosleep`,
`poll`, the Linux layer's waits) follows one pattern:

```
loop {
    let seq = task::wait_seq();      // before checking the condition
    if condition_met { break }
    if signal::interrupt_wait() { return EINTR }
    task::block_until(key, seq, deadline_ns);   // deadline 0 = none
}
```

- Producers change state, then call `task::wake(key)` (keys such as
  `key_pipe(id)`, `key_child(parent)`, `KEY_CONSOLE`). A signal wakes its
  target whatever it waits on.
- `wake` bumps a global sequence, and `block_until` refuses to block when
  it moved, so a wake between the check and the block is never lost. It
  first runs a task that wake queued for this CPU, if any: a caller that
  wakes a waiter of its own on every pass (a `poll` whose scan takes and
  lets go of a module's `SleepLock`, as sndd's does) would otherwise see
  its own wake, look again and never sleep, and with interrupts masked in
  its syscall nothing would preempt it for the waiter.
- Spurious wakes are fine: every caller re-checks its condition.
  `wait_addr` builds on this (`docs/threads.md`, "Waiting on an address").
- A lock whose holder may block (the VFS tree lock, a filesystem module's
  mount, a USB disk) is waited for the same way, never by spinning or by
  yielding in a loop. A syscall runs with interrupts masked, and
  `yield_now` keeps them so when nothing else is runnable; a device's
  interrupt goes to one CPU (CPU 0 on x86). A waiter looping on that CPU,
  the only one or one that runs user tasks, kept the holder's completion
  interrupt from arriving: the holder never woke, and the machine was
  dead, with CPU 0's cursor and keyboard (`get-myos` writing a USB stick
  on a laptop; `usb_fs_parallel` is the test). `block_until` halts with
  interrupts on when nothing else can run, so the wake arrives.

### Waiting for any event

`WAIT_ANY` waiters (`poll`, `nanosleep` with `SLEEP_ANY_EVENT`, the Linux
`ppoll`) are woken by every wake, and by `wake_any`, which runs after
device and file writes, the last close of a module file, and exits. That
is what lets `poll` and `netd` sleep instead of spinning. `block_until`
registers a `WAIT_ANY` waiter before it checks the sequence, so either
the producer sees the waiter or the waiter sees the bump.

### poll

`poll(fds, nfds, timeout_ms)` (`sys_poll`) asks each fd for readiness
(`task::fd_poll`) and sleeps on `WAIT_ANY`. Pipes and ptys report data,
room and hangup; the console tty committed input; regular files are
always ready; module files answer through `ModuleVfsOps::poll` (netfs:
socket data, hangup, connect, accept, room). A source without an
interrupt (a polled keyboard, a hook returning `MYOS_POLL_RECHECK`) is
re-checked every 10 ms. A module `read` returning `MYOS_READ_WAIT` makes
a read wait the same way. libgloss's `poll`, `select` and socket waits
are one call each.

## Timers and deadlines

Deadlines use `time::monotonic_ns()`: the TSC on x86_64 (rate from CPUID,
else calibrated against the PIT), `CNTVCT_EL0` on aarch64, the `time` CSR
on riscv64. All three arches tick at 100 Hz for preemption; each tick
scans the tasks only once the earliest deadline has passed.

Deadlines themselves are tickless: `block_until` pulls the CPU's timer in
to the new deadline (`arch::timer_deadline`), so a 1 ms `nanosleep` ends
after about 1 ms, not at the next 10 ms tick.

### Tickless idle

Idle CPUs are tickless (issue #367). A CPU that halts with nothing to run
stops its tick and arms its timer once, for the next deadline or at most
1 s ahead (`arch::timer_idle`); whatever ends the halt puts the tick back
(`arch::timer_resume`). The 1 s backstop bounds what a missed kick
costs. CPU 0 also wakes every 500 ms for the cursor blink while the text
console shows, and keeps its tick where the UART has no interrupt,
because that tick drains the UART.

Under single-threaded TCG (`MYOS_TCG_SINGLE=1`, `local-ci.sh`) an IPI to
a halted CPU, and a shootdown waiting for one, waits for QEMU's 100 ms
round-robin kick. So the boot tests use multi-threaded TCG on every arch.

## Console input

The UART's receive interrupt stages bytes into a lock-free ring
(`input::drain_uart_irq`) and wakes `KEY_CONSOLE`; the keyboard's
interrupt only wakes the reader, which takes the keys. Without those
interrupts, the BSP's tick stages the UART and the reader re-polls the
keyboard every 10 ms. Either way a starved shell cannot overrun the COM1
FIFO. The UARTs are programmed once (`SerialPort::new`), since
reprogramming them reset the FIFO and dropped input.

## History

- Re-homing every task after exec, or spreading children without the
  controlling-terminal check, broke the UEFI boot in CI; hence fork
  homes inherited by default.
- A shootdown ack given only from the IPI handler deadlocked a waiter
  with IRQs off; hence `tlb_service` from `schedule`.
- Full shootdowns after every map and unmap, and waits for halted CPUs'
  acks, doubled the boot time under `-smp 4` TCG; hence the
  loaded-elsewhere skip and `TLB_STALE`.
- A unix-socket write landing between a poller's scan and its `WAIT_ANY`
  registration was missed, and the X server stalled; hence registering
  before the sequence check.
- A 1 kHz tick cost a self-hosted build 10-14% under TCG; hence 100 Hz
  with tickless deadlines.
