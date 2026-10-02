# TODO

Performance follow-ups left open after the blocking scheduler (#187) and
virtio-net RX interrupts. Roughly in priority order.

## Tickless deadlines on aarch64 / riscv64

Sleep deadlines (`nanosleep`, `poll`/`select` timeouts, `NET_WAIT_RX`) are
only checked when the timer IRQ fires, and those arches tick at 100 Hz, so a
1 ms sleep lasts up to 10 ms. Either raise their tick to 1 kHz like x86 (one
constant each, ten times more idle wakeups) or, better, program the timer
(`cntv_tval_el0` / `stimecmp`) for the earliest deadline when it is sooner
than the next tick. `task::timer_tick` already keeps `NEXT_DEADLINE`.

## Per-key wait queues

`task::wake(key)` takes the global task lock and scans all 64 slots to find
the tasks blocked on that key. Fine at today's task counts; a list of waiters
per object (pipe, pty, child, console) would make wakes O(waiters) and keep
the scan out of the hot path once the task table grows.

## Idle-pull load balancing

User tasks are pinned to a home CPU chosen round-robin at spawn/fork (the
pinning is what keeps TLB flushes local). An idle CPU could steal a Ready task
whose home CPU is busy; needs a cross-CPU TLB shootdown on migration and the
NX #PF / leave races noted in `docs/pci-acpi-smp.md` resolved first.

## Rust std `thread::sleep`

The myos `std` sysroot has no `thread::sleep`; wire it to `SYS_NANOSLEEP`
(52). Touching `toolchain/std` triggers a sysroot rebuild in CI.
