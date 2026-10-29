# Scheduling, SMP and waits

## SMP model per architecture

All three arches use **Limine `MpRequest`**: the bootloader parks APs until
`MpInfo::bootstrap(ap_entry, logical_cpu)`. APs stay online into userspace.

| Arch | CPU id | AP init | Timer / IRQ | IPI |
|------|--------|---------|-------------|-----|
| x86_64 | TSC_AUX / APIC id | Per-CPU GDT+TSS, GS → syscall state, LAPIC timer (x2APIC MSRs when the CPU has an x2APIC, the xAPIC page otherwise; QEMU's TCG has no x2APIC, so CI runs the xAPIC path) | LVT timer → `schedule` | ICR all-excl-self (vec 33 TLB, 34 resched) |
| aarch64 | `TPIDR_EL1` / `MPIDR_EL1` | naked `goto_address` entry, the kernel TTBR0 and MAIR device attribute, `VBAR`/`use_spx`, the CPU's GIC bank (v2: banked GICC; v3: its redistributor + `ICC_*`), timers | PPI timer → `schedule` | GICv2 SGI 0 (TLB), SGI 1 (resched) |
| riscv64 | `tp` / Limine `hartid` | `stvec` / `sie` (STIE+SSIE) / `stimecmp` | S-mode timer → `schedule` | SBI IPI ext → SSIP; soft reason bits in `smp` |

Scheduler: per-CPU run queues + optional `affinity` (AP idle threads are
pinned; a user task's home moves when an idle CPU pulls it, above).
Kernel tasks use `affinity: None` (smoke `sched mask=0x3`). With more than one
CPU online, `spawn_user` round-robins user homes: **x86_64** and **aarch64**
over the APs (skip BSP), **riscv64** (`-smp 2`) over both harts. Fork
inherits the parent home for sequential fork+exec+wait and for init→getty
(init has no ctty; its long-lived `netd` child must not look like `make -j`).
If a **ctty-bearing** parent already has a Ready/Running child (interactive
`make -j` / pipelines), the new child takes a fresh AP RR home instead. Exec
keeps that affinity — no basename sticky/rehome allowlists and no blanket
post-exec RR (blanket re-home burned the UEFI 600s QEMU wall under CI; plain
`parent_has_active_child` without the ctty gate RR-spread getty and UEFI
page-faulted after fork exec). Cross-CPU exit reclaim is kept safe by: (1)
`die` enabling IRQs before reclaim/TLB shootdown, (2) epoch-based TLB
shootdown with soft `tlb_service` from `schedule` while IF-off (IRQ-only ACK
previously deadlocked a cli waiter), (3) x86 `flush_user_tlb` / unload skip
the remote IPI barrier when affinity pin guarantees the aspace was never
loaded elsewhere — full shootdowns after every map/unmap doubled MYOS_CI_MINI
under `-smp 4` TCG (600s wall); do not paper with a longer QEMU timeout.
A user task moves CPU only while it is Ready and off every CPU (a pull),
never while running (`affinity: None` user tasks stay off: the NX #PF /
leave races). `schedule` switches aspace/rsp0 before
publishing Ready (old stays Running across the CR3 write, lock not held during
switch); `unload_user_aspace` briefly kicks remotes then TLB-shootdowns; fork
queues the child for its home and kicks that CPU, or an idle one that may
pull the child while the parent keeps the home busy.
**aarch64** user tasks were BSP-pinned while block I/O waited on BSP-targeted
SPIs; virtio-blk/NVMe are polled now, so they take AP homes like x86 (the
`tlbi …is` flushes are inner-shareable broadcasts anyway). **riscv64** APs
join the scheduler too (`RISCV_PARK_APS = false`): the U-mode trap entry
reloads `tp` from the CPU-id footer the scheduler stamps at the base of the
task's kernel stack, so `cpu_id()` is right on every hart even though user
TLS lives in `tp`; the `sepc=0` corruption that once forced the WFI park was
the trap vector's t0 clobber (fixed in #179). The aarch64/riscv64 syscall
frame pointer used by fork/exec is a per-CPU cell (`SYSCALL_FRAMES`) that
`schedule` saves with the outgoing task and restores for the incoming one,
so a syscall that blocked and resumes (on any CPU) finds its own frame.
`note_schedule` → `/proc/cpuinfo`. QEMU `-smp 4` on x86/aarch64 (interactive
+ CI); riscv stays `-smp 2` (Limine panics `missing struct riscv_hart for
BSP` at 4). The packed `boot/virt.dtb` is always dumped with `-smp 2` so
OpenSBI BSP hartid=1 still has a hart node. The WFI park stub stays compiled
for riscv64 (flip `RISCV_PARK_APS` to retreat).
On every arch the BSP timer drains UART RX into a staging ring
(`input::drain_uart_irq`) and wakes the console reader (`KEY_CONSOLE`), so a
starved shell under `-smp 4` TCG cannot overrun the COM1 FIFO / sticky-key
login and a blocked reader never polls the UART itself.

Per-CPU ring3↔ring0 state:

- **x86_64** — each CPU has its own GDT+TSS (`rsp0` / DF IST). `IA32_EFER.NXE`
  and `IA32_GS_BASE` → `CpuSyscallState` are programmed on BSP and every AP
  (`kernel_rsp0` + fork callee snapshot). User CS/SS come from the CPU's GDT.
- **aarch64** — banked `SP_ELx` after `use_spx`; exception frames live on the
  current task's kernel stack. Limine `goto_address` publishes via STLR+DC CVAC;
  APs take a naked entry (FPEN + progress flag) then install the kernel TTBR0 and the MAIR device attribute before the GIC (the device window is in the shared TTBR1).
- **riscv64** — `sscratch` holds the current task's kernel stack top (updated on
  every schedule). Per-hart cells are deferred until multi-hart Limine bring-up
  is reliable on QEMU.

TLB shootdown: local invalidate, then IPI the other online CPUs and wait
briefly for acks (`smp::tlb_shootdown`, try-lock + bounded spin). An idle
CPU (in the idle loop, or halted) is passed over: it runs no user code, so
it is flagged instead (`TLB_STALE`) and flushes its TLB when it next
resumes a task (`schedule`), and its ack is given for it. Waiting for a
halted CPU's ack cost its wake per shootdown (under TCG the host thread's),
for every copy-on-write fork and exit of a shell command. A targeted
reschedule IPI wakes a halted CPU when a task homed there becomes Ready
(`spawn`, `fork`, `wake`).

AP bring-up: IRQs stay masked and `ONLINE` is clear until `ap_idle_loop`
installs `CURRENT` and migrates onto the AP's own 64KiB idle stack (Limine
AP stacks are too small for nested timer/IPI frames).

## Blocking waits, idle CPUs and wakeups

Tasks have a `Blocked` state. Every kernel wait — `wait`/`waitpid`, console
`read`, pipe/FIFO/pty read/write/open, `sigsuspend`/`sigwait`, `nanosleep`,
`poll`, the Linux layer's `nanosleep`/`ppoll` — follows one pattern:

```
loop {
    let seq = task::wait_seq();      // before checking the condition
    if condition_met { break }
    if signal::interrupt_wait() { return EINTR }
    task::block_until(key, seq, deadline_ns);   // deadline 0 = none
}
```

Producers change state and call `task::wake(key)` (`key_pipe(id)`,
`key_pty(id)`, `key_child(parent)`, `KEY_CONSOLE`); `signal_send` wakes the
target whatever it waits on (`wake_task`). `wake` bumps a global sequence
under `TASKS`, and `block_until` refuses to block when the sequence moved,
so a wake between the check and the block is never lost; spurious wakes are
fine because every caller re-checks. `WAIT_ANY` waiters (`SYS_POLL`,
`SYS_NANOSLEEP` with `SLEEP_ANY_EVENT`, the Linux `ppoll`) are woken by
every wake and by `wake_any` after device/file writes, the last close of a
module file and exits — that is what lets `poll` and `netd` sleep instead of
spinning on `gettimeofday`. `wake_any` skips the task scan when no `WAIT_ANY`
waiter is registered, but bumps the sequence first, and `block_until`
registers a `WAIT_ANY` waiter before it checks the sequence: either the
producer sees the waiter or the waiter sees the bump. (Before, a unix-socket
write landing between a poller's scan and its registration was missed until
some other event: the X server then left a client's requests unanswered.)

`SYS_POLL` (`poll(fds, nfds, timeout_ms)`, `sys_poll`) scans a `struct
pollfd` array with `task::fd_poll` and sleeps on `WAIT_ANY` until the first
fd is ready, the timeout, or a signal. Readiness comes from each object:
pipes and ptys (data, room, the peer gone), the console tty (committed
input; a keyboard without an interrupt is polled, so a watched tty then
re-checks every 10 ms), regular files (always ready), and module files through
the optional `ModuleVfsOps::poll` hook (ABI 17): netfs reports a socket's
bytes, hangup, finished connect, queued accept and, for `/net/unix`, room
in the peer's buffer; a hook that adds `MYOS_POLL_RECHECK` is re-checked
every 10 ms like the tty (`/dev/console/kbd` when its keyboard has no
interrupt, ABI 20),
and a module `read` that returns `MYOS_READ_WAIT` makes an fd read wait for
the file the same way. libgloss's `poll`/`select` are one call, and so is
every blocking wait of its socket library (`pollselect.c`, `socket.c`).

Deadlines use `time::monotonic_ns()` (x86: TSC at the rate CPUID leaf 15H/16H states, else calibrated against the PIT
channel 2 at boot; aarch64: `CNTVCT_EL0`; riscv64: `time` CSR). Each timer
IRQ calls `task::timer_tick()`, which only scans `TASKS` once the earliest
deadline (`NEXT_DEADLINE`) has passed. The x86 LAPIC tick is calibrated
against that clock to 100 Hz (it used to fire 10-20k times per second per
CPU, each tick taking the scheduler lock; at 1 kHz the ticks cost a
self-hosted core+alloc build 10-14% under TCG). All three tick at 100 Hz
but are tickless for deadlines: `block_until` calls
`arch::timer_deadline`, which pulls the CPU's own timer in to the new
deadline (aarch64/riscv64: `cntv_cval_el0` / `stimecmp`, absolute compare
values; x86: the periodic LAPIC timer's current count, cut short and put
back at the tick), and every re-arm programs `min(next tick,
task::next_deadline_ns())`, so a 1 ms
`nanosleep` ends after about 1 ms instead of at the next 10 ms boundary.

Idle CPUs are tickless (issue #367): a CPU that halts with
nothing to run (`sched::halt`, from the idle loop and `block_until`) stops
its tick and arms its timer once, for `NEXT_DEADLINE` or at most 1 s ahead
(`arch::timer_idle`: x86 puts the LAPIC timer in one-shot mode, aarch64
turns the physical timer off and sets `cntv_cval_el0`, riscv64 sets
`stimecmp`), so an idle CPU is not woken 100 times a second. The 1 s
backstop bounds what a wakeup that was never kicked costs. Whatever ends the
halt and runs something puts the tick back (`arch::timer_resume`, from
`halt` and from `schedule`, since an interrupt may switch tasks from inside
the halt): a busy CPU keeps its 100 Hz tick for preemption. CPU 0 also
wakes for the cursor blink (every 500 ms, while the screen shows the text
console), and keeps its tick where the UART has no interrupt (no I/O APIC):
that tick drains the UART then (`input::tick`). Under single-threaded TCG (`MYOS_TCG_SINGLE=1`, `local-ci.sh`) an IPI
to a halted CPU waits for QEMU's 100 ms round-robin kick, since one host
thread runs every vCPU; TLB shootdowns waiting for a tickless CPU's ack are
then that slow, so the boot tests use multi-threaded TCG on every arch. Each idle CPU's halts then last up to its next deadline (the
`idle_tickless` test checks a quiet CPU averages over 20 ms a halt,
`idle_cpu0` the same of CPU 0).

Run queues: the Ready tasks that are on no CPU sit in per-CPU run queues
(`sched::RUNQ`, a bit per task slot: the tasks homed on that CPU) and one
queue of the tasks that may run anywhere (`FLOAT`, the kernel's threads);
the idle tasks are never queued. The bits change with the task states
under `TASKS` (a task is queued when it is made Ready off a CPU: a wake, a
spawn, `finish_switch` for a preempted task), so picking the next task is a
few bit operations on the CPU's queue and the floating one, round-robin
after the current slot, and "is anything ready here?" is a lock-free load
(the old pick walked all 64 slots under `TASKS` on every tick, switch and
yield, and again before every halt).

Idle: `kernel_main` (task 0) is the BSP's idle task, `ap_idle_body` the APs'.
`idle_step` is one pass of the scheduler: the CPU marks itself idle
(`CPU_IDLE`), picks under `TASKS` (its queue, the floating queue, else a
pull, below) and, when that found nothing, halts (`arch::idle_wait`: `sti;
hlt` / `wfi` with the pending-interrupt wakeup) unless `NEED_RESCHED[cpu]`
was set or its queue filled meanwhile: a task queued for an idle CPU after
its pick sets the flag and sends it an IPI, which is pending when it
halts. A task that blocks with nothing else runnable halts the same way on
its own stack, from the same single pass (`block_until`); `schedule` never
picks a woken task that is still some CPU's `CURRENT` (it is not queued),
and a wake that lands while the task is mid-switch (`SWITCHED_FROM`) is
deferred to `finish_switch` (`wake_pending`). A task leaving a CPU stays
Running until `finish_switch`, also one a wake already made Ready while it
halted there: left Ready, a peer could resume it from its previous, stale
switch frame before `task_switch` saved the new one. `wake` queues the
woken task for its home CPU (or marks the CPU it is halting on), and sends
a **targeted** reschedule IPI (`IrqChip::send_ipi` to one CPU: the LAPIC ICR with
a destination, a GIC SGI to one CPU (v2: a CPUTargetList bit, v3: the affinity in `ICC_SGI1R_EL1`), SBI IPI with a single
hart) only when that CPU is halted.

Idle-pull balancing (`sched::pull`): a CPU with nothing in its queues, and
whose current task cannot go on, takes a user task from the queue of a busy
CPU (one that is not idle: an idle CPU runs its own queue as soon as it is
kicked) and makes itself its home. A user task so keeps a home, runs on one
CPU at a time and is loaded on that CPU only (`LOADED_ASPACE`): the task is
off every CPU while it waits, and the CPU it left switched to another root
before `finish_switch` queued it, which flushed its TLB, so the TLB rules
for threads (`docs/threads.md`) cover a pulled task too. A task queued for
a busy home, or a floating task, kicks an idle CPU that may run it
(`kick_for`), so the pull happens at once rather than at the home's next
tick; a kernel thread is never pulled (it floats), nor is a user task onto
the BSP where `user_affinity` keeps user tasks off it (x86_64, aarch64).
`/proc/cpuinfo` counts the pulls per CPU. The UART's receive interrupt stages its
bytes (`input::drain_uart_irq`, into a lock-free ring) and wakes
`KEY_CONSOLE`; the keyboard's only wakes it (`KernelApi::console_input`),
and the reader takes the keys. Without those interrupts the BSP's tick
stages UART RX and the console reader re-polls the keyboard every 10 ms.

The UARTs are programmed once (`SerialPort::new` used to reprogram COM1 /
the PL011 on every output byte; the FIFO-reset bits in that sequence dropped
input that arrived while the kernel echoed, which only showed once the drain
tick slowed down).

`/proc/cpuinfo` reports `schedules`, `idle_halts` and `pulls` per CPU,
`blocked_tasks`, `clock_hz` and `uptime_ms` — an idle shell should show
`idle_halts` climbing and `schedules` nearly flat.
