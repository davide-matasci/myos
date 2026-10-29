# Scheduling, SMP and waits

This page covers how myos runs tasks on several CPUs: how the CPUs come
up on each architecture, how the scheduler picks and places tasks, how a
task blocks and is woken, and how timers and idle CPUs behave. Read it
before changing `kernel/src/task/sched.rs`, `kernel/src/smp.rs`, a wait
in a syscall or an arch's timer code. Threads, process lifetime and the
TLB rules for an address space loaded on several CPUs are in
`docs/threads.md`.

## Watching it

`/proc/cpuinfo` reports per CPU `schedules`, `idle_halts` and `pulls`
(fed by `note_schedule`), and overall `blocked_tasks`, `clock_hz` and
`uptime_ms`. An idle shell should show `idle_halts` climbing and
`schedules` nearly flat.

QEMU runs `-smp 4` on x86_64 and aarch64, interactive and in CI. riscv64
stays at `-smp 2`, because Limine panics with `missing struct riscv_hart
for BSP` at 4. The packed `boot/virt.dtb` is always dumped with `-smp 2`,
so that OpenSBI's BSP, hartid 1, still has a hart node.

## CPUs per architecture

All three arches use Limine's `MpRequest`. The bootloader parks the APs
until `MpInfo::bootstrap(ap_entry, logical_cpu)`, and the APs stay online
into userspace.

| Arch | CPU id | AP init | Timer / IRQ | IPI |
|------|--------|---------|-------------|-----|
| x86_64 | TSC_AUX / APIC id | Per-CPU GDT+TSS, GS → syscall state, LAPIC timer (x2APIC MSRs when the CPU has an x2APIC, the xAPIC page otherwise; QEMU's TCG has no x2APIC, so CI runs the xAPIC path) | LVT timer → `schedule` | ICR all-excl-self (vec 33 TLB, 34 resched) |
| aarch64 | `TPIDR_EL1` / `MPIDR_EL1` | naked `goto_address` entry, the kernel TTBR0 and MAIR device attribute, `VBAR`/`use_spx`, the CPU's GIC bank (v2: banked GICC; v3: its redistributor + `ICC_*`), timers | PPI timer → `schedule` | GICv2 SGI 0 (TLB), SGI 1 (resched) |
| riscv64 | `tp` / Limine `hartid` | `stvec` / `sie` (STIE+SSIE) / `stimecmp` | S-mode timer → `schedule` | SBI IPI ext → SSIP; soft reason bits in `smp` |

### AP bring-up

An AP keeps its IRQs masked, and its `ONLINE` bit clear, until
`ap_idle_loop` installs `CURRENT` and moves onto the AP's own 64 KiB idle
stack. Limine's AP stacks are too small for nested timer and IPI frames.

### Per-CPU kernel entry state

- **x86_64**: each CPU has its own GDT and TSS (`rsp0`, the DF IST).
  `IA32_EFER.NXE` and `IA32_GS_BASE` → `CpuSyscallState` are programmed on
  the BSP and on every AP (`kernel_rsp0` and the fork callee snapshot).
  User CS/SS come from the CPU's GDT.
- **aarch64**: `SP_ELx` is banked after `use_spx`, and exception frames
  live on the current task's kernel stack. Limine's `goto_address` is
  published with STLR+DC CVAC. An AP takes a naked entry (FPEN and a
  progress flag), then installs the kernel TTBR0 and the MAIR device
  attribute before the GIC. The device window is in the shared TTBR1.
- **riscv64**: `sscratch` holds the current task's kernel stack top,
  updated on every schedule. User TLS lives in `tp`, so the U-mode trap
  entry reloads `tp` from a CPU-id footer that the scheduler stamps at the
  base of the task's kernel stack. That keeps `cpu_id()` right on every
  hart.

On aarch64 and riscv64 the syscall frame pointer that fork and exec use is
a per-CPU cell (`SYSCALL_FRAMES`). `schedule` saves it with the outgoing
task and restores it for the incoming one, so a syscall that blocked and
resumes, on any CPU, finds its own frame.

## Run queues and home CPUs

The scheduler keeps per-CPU run queues and an optional `affinity` per
task, the task's home CPU.

- **Run queues.** Ready tasks that are on no CPU sit in per-CPU queues
  (`sched::RUNQ`, one bit per task slot: the tasks homed on that CPU) and
  in one queue of tasks that may run anywhere (`FLOAT`, the kernel's
  threads). Idle tasks are never queued.
- **Queueing.** The bits change with the task states, under `TASKS`. A
  task is queued when it becomes Ready while off a CPU: a wake, a spawn,
  or `finish_switch` for a preempted task.
- **Picking.** Picking the next task is a few bit operations on the CPU's
  queue and the floating one, round-robin after the current slot. "Is
  anything ready here?" is a lock-free load.

### Where tasks run

- Kernel tasks use `affinity: None` and float (the smoke prints
  `sched mask=0x3`). AP idle threads are pinned.
- With more than one CPU online, `spawn_user` gives each new user task a
  home round-robin (`user_affinity` in `kernel/src/task/lifecycle.rs`).
  On x86_64 and aarch64 it goes over the APs and skips the BSP. On riscv64
  (`-smp 2`) it goes over both harts. Threads get homes the same way
  (`docs/threads.md`).
- `fork` gives the child its parent's home. This keeps a sequential
  fork+exec+wait on one CPU, and keeps init's children with it: init has
  no controlling terminal, and its long-lived `netd` child must not look
  like a `make -j`.
- If a parent with a controlling terminal already has an active (Ready,
  Running or Blocked) child, as with an interactive `make -j` or a
  pipeline, the new child takes a fresh round-robin home instead
  (`parent_has_active_child`).
- `exec` keeps the task's affinity. There are no per-program rules and no
  re-homing after exec.
- A user task changes CPU only while it is Ready and off every CPU (a
  pull, below), never while it runs. User tasks never have `affinity:
  None`, because of the NX #PF and `leave` races.
- `fork` queues the child for its home and kicks that CPU, or kicks an
  idle CPU that may pull the child while the parent keeps the home busy.
- On aarch64, user tasks take AP homes like on x86: virtio-blk and NVMe
  are polled, and the `tlbi …is` flushes are inner-shareable broadcasts
  anyway. On riscv64 the APs join the scheduler too.

### Idle-pull balancing

`sched::pull` balances the load. A CPU with nothing in its queues, whose
current task cannot go on, takes a user task from the queue of a busy CPU
and becomes that task's home. A busy CPU is one that is not idle: an idle
CPU runs its own queue as soon as it is kicked.

So a user task always has a home, runs on one CPU at a time, and is
loaded on that CPU only (`LOADED_ASPACE`). While it waits, the task is off
every CPU, and the CPU it left had switched to another root before
`finish_switch` queued it, which flushed its TLB. The TLB rules for
threads (`docs/threads.md`) therefore cover a pulled task too.

- A task queued for a busy home, or a floating task, kicks an idle CPU
  that may run it (`kick_for`), so the pull happens at once rather than at
  the home's next tick.
- A kernel thread is never pulled: it floats.
- A user task is never pulled onto the BSP where `user_affinity` keeps
  user tasks off it (x86_64, aarch64).
- `/proc/cpuinfo` counts the pulls per CPU.

### Switching

`schedule` switches the address space and `rsp0` before it publishes the
old task as Ready. The old task stays Running across the CR3 write, and
the lock is not held during the switch.

- `schedule` never picks a woken task that is still some CPU's `CURRENT`
  (such a task is not queued).
- A wake that lands while the task is mid-switch (`SWITCHED_FROM`) is
  deferred to `finish_switch` (`wake_pending`).
- A task leaving a CPU stays Running until `finish_switch`, even one that
  a wake already made Ready while it halted there. Left Ready, a peer could
  resume it from its previous, stale switch frame before `task_switch`
  saved the new one.

## Idle CPUs

`kernel_main` (task 0) is the BSP's idle task, and `ap_idle_body` is the
APs'. `idle_step` is one pass of the scheduler:

1. The CPU marks itself idle (`CPU_IDLE`).
2. Under `TASKS` it picks from its queue, then the floating queue, else
   tries a pull.
3. When that found nothing, it halts (`arch::idle_wait`: `sti; hlt` or
   `wfi`, with the pending-interrupt wakeup), unless `NEED_RESCHED[cpu]`
   was set or its queue filled meanwhile.

A task queued for an idle CPU after that CPU's pick sets the flag and
sends it an IPI, which is still pending when the CPU halts. A task that
blocks with nothing else runnable halts the same way, on its own stack,
from the same single pass (`block_until`).

`wake` queues the woken task for its home CPU, or marks the CPU it is
halting on. It sends a **targeted** reschedule IPI only when that CPU is
halted (`IrqChip::send_ipi` to one CPU):

- x86_64: the LAPIC ICR with a destination;
- aarch64: a GIC SGI to one CPU (v2: a CPUTargetList bit; v3: the affinity
  in `ICC_SGI1R_EL1`);
- riscv64: an SBI IPI with a single hart.

## TLB shootdown

`smp::tlb_shootdown` invalidates locally, then sends an IPI to the other
online CPUs and waits briefly for their acks (try-lock and a bounded
spin).

- An idle CPU (in the idle loop, or halted) is passed over. It runs no
  user code, so it is flagged instead (`TLB_STALE`) and flushes its TLB
  when it next resumes a task (`schedule`); its ack is given for it.
- A targeted reschedule IPI wakes a halted CPU when a task homed there
  becomes Ready (`spawn`, `fork`, `wake`).

Exit reclaims an address space safely across CPUs because:

1. `die` enables IRQs before the reclaim and its TLB shootdown;
2. the shootdown is epoch-based, and a CPU waiting with IRQs off still
   serves it: `schedule` calls `tlb_service` in software;
3. x86's `flush_user_tlb` and the unload skip the remote IPI barrier when
   no other CPU has the address space loaded
   (`task::aspace_loaded_elsewhere`). `unload_user_aspace` briefly kicks
   the remote CPUs, then shoots their TLBs down.

Do not paper over a slow shootdown path with a longer QEMU timeout.

## Blocking waits

A task gives up its CPU in one of three ways: it calls `task::yield_now()`,
the timer interrupt preempts it (the handler calls `task::schedule()` after
the EOI, in user mode too), or it blocks in a wait.

Tasks have a `Blocked` state. Every kernel wait follows one pattern:
`wait`/`waitpid`, the console `read`, pipe, FIFO and pty reads, writes and
opens, `sigsuspend`/`sigwait`, `nanosleep`, `poll`, and the Linux layer's
`nanosleep`/`ppoll`.

```
loop {
    let seq = task::wait_seq();      // before checking the condition
    if condition_met { break }
    if signal::interrupt_wait() { return EINTR }
    task::block_until(key, seq, deadline_ns);   // deadline 0 = none
}
```

- Producers change state, then call `task::wake(key)`. The keys include
  `key_pipe(id)`, `key_pty(id)`, `key_child(parent)` and `KEY_CONSOLE`.
- `signal_send` wakes the target whatever it waits on (`wake_task`).
- `wake` bumps a global sequence under `TASKS`, and `block_until` refuses
  to block when the sequence moved. So a wake between the check and the
  block is never lost.
- Spurious wakes are fine, because every caller re-checks its condition.
  `wait_addr` builds on this (`docs/threads.md`, "Waiting on an address").

### Waiting for any event

`WAIT_ANY` waiters are woken by every wake. These are `SYS_POLL`,
`SYS_NANOSLEEP` with `SLEEP_ANY_EVENT`, and the Linux `ppoll`. They are
also woken by `wake_any`, which runs after device and file writes, the
last close of a module file, and exits. This is what lets `poll` and
`netd` sleep instead of spinning on `gettimeofday`.

`wake_any` skips the task scan when no `WAIT_ANY` waiter is registered,
but it bumps the sequence first. `block_until` registers a `WAIT_ANY`
waiter before it checks the sequence. So either the producer sees the
waiter, or the waiter sees the bump.

### poll

`SYS_POLL` (`poll(fds, nfds, timeout_ms)`, `sys_poll`) scans a `struct
pollfd` array with `task::fd_poll`. It sleeps on `WAIT_ANY` until the
first fd is ready, the timeout passes, or a signal arrives. Each kind of
object reports its own readiness:

- pipes and ptys: data, room, the peer gone;
- the console tty: committed input. A keyboard without an interrupt is
  polled, so a watched tty then re-checks every 10 ms;
- regular files: always ready;
- module files: through the optional `ModuleVfsOps::poll` hook (ABI 17).
  netfs reports a socket's bytes, hangup, a finished connect, a queued
  accept and, for `/net/unix`, room in the peer's buffer. A hook that adds
  `MYOS_POLL_RECHECK` is re-checked every 10 ms like the tty
  (`/dev/console/kbd` when its keyboard has no interrupt, ABI 20).

A module `read` that returns `MYOS_READ_WAIT` makes an fd read wait for
the file the same way. libgloss's `poll` and `select` are one call each,
and so is every blocking wait of its socket library (`pollselect.c`,
`socket.c`).

## Timers and deadlines

Deadlines use `time::monotonic_ns()`:

- x86_64: the TSC, at the rate CPUID leaf 15H/16H states, else calibrated
  against PIT channel 2 at boot;
- aarch64: `CNTVCT_EL0`;
- riscv64: the `time` CSR.

All three arches tick at 100 Hz. The x86 LAPIC tick is calibrated against
that clock. Each timer IRQ calls `task::timer_tick()`, which scans `TASKS`
only once the earliest deadline (`NEXT_DEADLINE`) has passed.

Deadlines themselves are tickless. `block_until` calls
`arch::timer_deadline`, which pulls the CPU's own timer in to the new
deadline:

- aarch64 and riscv64: `cntv_cval_el0` / `stimecmp`, absolute compare
  values;
- x86_64: the periodic LAPIC timer's current count, cut short and put
  back at the tick.

Every re-arm programs `min(next tick, task::next_deadline_ns())`. So a
1 ms `nanosleep` ends after about 1 ms, not at the next 10 ms boundary.

### Tickless idle

Idle CPUs are tickless (issue #367). A CPU that halts with nothing to run
(`sched::halt`, from the idle loop and from `block_until`) stops its tick.
It arms its timer once, for `NEXT_DEADLINE` or at most 1 s ahead
(`arch::timer_idle`), so an idle CPU is not woken 100 times a second:

- x86_64 puts the LAPIC timer in one-shot mode;
- aarch64 turns the physical timer off and sets `cntv_cval_el0`;
- riscv64 sets `stimecmp`.

The 1 s backstop bounds what a wakeup that was never kicked costs.
Whatever ends the halt and runs something puts the tick back
(`arch::timer_resume`). It is called from `halt` and from `schedule`,
since an interrupt may switch tasks from inside the halt. A busy CPU keeps
its 100 Hz tick for preemption.

CPU 0 has two exceptions:

- it also wakes for the cursor blink, every 500 ms, while the screen shows
  the text console;
- it keeps its tick where the UART has no interrupt (no I/O APIC), because
  that tick drains the UART (`input::tick`).

Each idle CPU's halts last up to its next deadline. The `idle_tickless`
test checks that a quiet CPU averages over 20 ms per halt, and
`idle_cpu0` checks the same of CPU 0.

Under single-threaded TCG (`MYOS_TCG_SINGLE=1`, `local-ci.sh`) one host
thread runs every vCPU. An IPI to a halted CPU then waits for QEMU's
100 ms round-robin kick, and so does a TLB shootdown waiting for a
tickless CPU's ack. For that reason the boot tests use multi-threaded TCG
on every arch.

## Console input

The UART's receive interrupt stages its bytes into a lock-free ring
(`input::drain_uart_irq`) and wakes `KEY_CONSOLE`. The keyboard's
interrupt only wakes it (`KernelApi::console_input`), and the reader takes
the keys. A blocked reader never polls the UART itself.

Without those interrupts, the BSP's timer tick stages UART RX into the
same ring and wakes the console reader, and the reader re-polls the
keyboard every 10 ms. Either way, a shell starved under `-smp 4` TCG
cannot overrun the COM1 FIFO or get sticky keys at the login.

The UARTs are programmed once (`SerialPort::new`).

## History

- Re-homing every task after exec burned the UEFI boot's 600 s QEMU wall
  in CI. Spreading children with `parent_has_active_child` but without the
  controlling-terminal check spread getty too, and UEFI page-faulted after
  fork+exec. Hence the inherit-by-default fork homes above.
- A shootdown ack given only from the IPI handler deadlocked a waiter with
  IRQs off; hence `tlb_service` from `schedule`.
- Full shootdowns after every map and unmap doubled `MYOS_CI_MINI` under
  `-smp 4` TCG and hit the 600 s wall; hence the skip when no other CPU has
  the address space loaded.
- Waiting for a halted CPU's ack cost that CPU's wakeup (under TCG, the
  host thread's) on every shootdown, for every copy-on-write fork and exit
  of a shell command; hence `TLB_STALE`.
- aarch64 user tasks were pinned to the BSP while block I/O waited on
  SPIs targeted at the BSP, until virtio-blk and NVMe became polled.
- riscv64 once parked its APs in WFI because of a `sepc=0` corruption. The
  cause was the trap vector clobbering t0 (fixed in #179).
- A unix-socket write that landed between a poller's scan and its
  `WAIT_ANY` registration was missed until some other event, and the X
  server left a client's requests unanswered. Hence registering before the
  sequence check.
- The x86 LAPIC tick fired 10-20k times per second per CPU, each tick
  taking the scheduler lock. Even at 1 kHz the ticks cost a self-hosted
  core+alloc build 10-14% under TCG; hence 100 Hz with tickless deadlines.
- The old pick walked all 64 task slots under `TASKS` on every tick,
  switch and yield, and again before every halt; hence the bit-set run
  queues.
- `SerialPort::new` used to reprogram COM1 or the PL011 on every output
  byte. The FIFO-reset bits in that sequence dropped input that arrived
  while the kernel echoed, which only showed once the drain tick slowed
  down.
