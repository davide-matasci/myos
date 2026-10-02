# TODO

Follow-ups left open after the blocking scheduler (#187), virtio-net RX
interrupts (#188) and the modular kernel (arch/ split, process blocks, every
driver a module). Roughly in priority order.

## PCI interrupt routing from firmware tables (real hardware)

`pci_irq_setup` hardcodes QEMU `virt`'s wiring: INTx → GIC SPI 3 + (slot +
pin − 1) mod 4 on aarch64, PLIC source 32 + the same swizzle on riscv64, and
the GIC/PLIC base addresses. A real board publishes this in the device tree:
walk the PCIe host bridge's `interrupt-map` / `interrupt-map-mask` for the
device's bus/slot/pin (Limine already hands us the DTB) and take controller
bases from the same tree. On x86_64 MSI-X is the primary path and portable;
INTx would need `_PRT` from the DSDT plus an IOAPIC driver, neither of which
exists (the ACPI module only scans for `_S5`). Interrupt remapping (IOMMU) and
x2APIC destinations above 255 CPUs are also unhandled. A wrong mapping today
degrades to netd's 1 s `NET_WAIT_RX` backstop rather than breaking.

## Module follow-ups

- **Device rescan / hotplug**: drivers probe once in `module_init`. Writing
  `rescan` to `/proc/pci` used to re-probe the in-kernel NVMe driver; now it
  only refreshes the listing. A `rescan` hook in `ModuleBlkOps` (or a generic
  module op the pci_enum writer calls) would restore that.
- **`rmmod`**: `module_exit` exists but nothing calls it; unloading needs
  unregister paths for blk/chr/fs/console ops and a check that no fd or
  mount still uses them.
- **Per-process locks**: the `Process` blocks (`task/process.rs`) hang off
  the one `TASKS` lock. Giving each its own lock (TASKS → process ordering)
  would let fd/mmap syscalls of different processes stop contending.
- **virtio-mmio discovery from the DTB**: the `virtio_blk` and `console`
  modules scan QEMU `virt`'s fixed MMIO window; real boards need the
  `virtio,mmio` nodes from the device tree.
- **Module dependencies**: the boot order in `BOOT_MODULES` is the only
  ordering (console first, block drivers before filesystems). A module could
  declare what it needs (`blk_count() > 0`, another module's name) so
  `insmod` can refuse or defer.

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
