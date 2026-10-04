# PCI discovery, ACPI (custom AML), and SMP scheduler

## Module / kernel split

| Component | Location | Why |
|-----------|----------|-----|
| PCI config access (`cfg_read32` / BAR map / `pci_find` / `pci_find_class`) | `kernel/src/pci.rs` + `arch/*/pci.rs` | The module ABI exposes it; every PCI driver is a module |
| virtio-blk, NVMe | `modules/virtio_blk`, `modules/nvme` | Register `/dev/<name>` through `blk_register`; loaded before the filesystem modules |
| Framebuffer text, keyboards, keymap | `modules/console` | Serial stays in the kernel; the module registers screen + keyboard ops (`console_register`) |
| Linux syscall layer | `modules/linux` | Registers a syscall *personality* (`personality_register`); the kernel keeps only which tasks have it (`kernel/src/personality.rs`) |
| Full PCI enumeration → `/proc/pci` | `modules/pci_enum` (`.ko`) | Discovery + on-demand rescan via write; talks only through `KernelApi` |
| ACPI tables + AML → `/proc/acpi/*` | `modules/acpi` (`.ko`) | Optional; stubs when no RSDP |
| Limine RSDP / MP requests | `kernel/src/limine_boot.rs` | Boot protocol |
| AP bring-up + `/proc/cpuinfo` | `kernel/src/smp.rs` | Must run before modules; owns CPU-local state + IPI helpers |
| Cross-CPU scheduler | `kernel/src/task/` | Per-CPU `CURRENT`, task `affinity`, shared ready set |
| Proc exporters | `kernel/src/fs/procfs.rs` | Built-ins: `mounts`, `cpuinfo`; dynamic via `proc_register` ABI |

ABI version: **14** (`dt_mmio_find`: a module finds its memory-mapped devices in the device tree; 13: `personality_register`, `personality_exec`, `native_syscall` and the task / fd / VFS / signal / FPU / wait helpers a syscall personality needs: the Linux layer is a module; 12: `blk_register`, `pci_find_class`, `framebuffer_info`, `console_register`: block devices and the console are modules; 11: `pci_irq_enable`, `wake_any`, `wait_seq`, `block_until`, `monotonic_ns` for device interrupts and blocking waits; 10: `proc_set_writer` for `/proc/pci` rescan; earlier: `proc_register`, `acpi_rsdp`, `hhdm_offset`).

Boot modules come from Limine's module list (`limine.conf` `module_path` entries, `src/limine_image.rs` `BOOT_MODULES` plus the `OPTIONAL_MODULES` whose Cargo feature is on, in load order); `insmod <path>` (`SYS_INSMOD`) loads more at runtime and `/proc/modules` lists them.

## Device interrupts

`kernel/src/irq.rs` keeps one handler per interrupt number and `pci_irq_setup`
picks the delivery mechanism per arch, so a module only asks for "this PCI
function's interrupt" (`KernelApi::pci_irq_enable`) and gets told whether to
use an MSI-X table entry or the INTx line:

| Arch | Mechanism | Numbering |
|------|-----------|-----------|
| x86_64 | MSI-X entry 0 → LAPIC vector on the BSP (`MSI_VECTOR_BASE` 48 + n, 8 vectors; no IOAPIC / PIRQ routing) | vector |
| aarch64 | INTx → GICv2 SPI, level, priority 0x80, CPU 0; the SPI comes from the device tree's PCIe `interrupt-map` (QEMU `virt`: SPI 3 + (slot + pin − 1) mod 4) | INTID (35..38) |
| riscv64 | INTx → PLIC source for the boot hart's S-mode context, from the same `interrupt-map` (`virt`: 32 + (slot + pin − 1) mod 4), `sie.SEIE` | PLIC source |

## The device tree (aarch64, riscv64)

Nothing about the board is hard-coded on these arches: `kernel/src/dt.rs`
parses the flattened device tree Limine hands over (the `fdt` crate,
MPL-2.0) and `arch::apply_dt` runs before the first console output:

| Node (`compatible`) | Used for |
|------|------|
| `arm,cortex-a15-gic` / `arm,gic-400` | GICv2 distributor + CPU interface bases |
| `sifive,plic-1.0.0` / `riscv,plic0` | PLIC base |
| `arm,pl011` / `ns16550a` | console UART base |
| `arm,pl031` / `google,goldfish-rtc` | RTC base (optional: no RTC, no wall clock) |
| `/cpus` `timebase-frequency` | riscv64 `time` CSR rate (monotonic clock, timer) |
| `pci-host-ecam-generic` | ECAM window and `bus-range`; the 32-bit (aarch64) or 64-bit (riscv64) MMIO `ranges` entry BARs are assigned from; `interrupt-map` / `interrupt-map-mask` for INTx |
| `virtio,mmio` | the virtio-mmio transports, for modules through `KernelApi::dt_mmio_find` (ABI 14), in ascending address order |

A missing tree or node stops the boot with `fatal: device tree: ...` on the
UART at QEMU `virt`'s address (the one assumption left, so the message has
somewhere to go). The EDK2 firmware boots (AAVMF, RISC-V EDK2) hand Limine
no tree, so the host tool dumps QEMU's (`-machine ...,dumpdtb`, same machine
options and `-smp` as the boot) into the ESP as `boot/virt-aarch64.dtb` /
`boot/virt.dtb` and `limine.conf` passes it with `global_dtb`. Interrupt specifiers are decoded per arch
(`arch::irq_from_dt`: GIC `<type number flags>` → INTID, PLIC `<source>`).
x86_64 has no tree (ACPI): `dt::init` finds none and the arch ignores it.

Handlers run in interrupt context with interrupts masked on CPU 0. On INTx
the handler must read the device's ISR register so the level line deasserts
before the GIC/PLIC EOI. `/proc/interrupts` lists counts per registered
interrupt plus spurious ones.

virtio-net uses this for its RX queue: `AVAIL_F_NO_INTERRUPT` is cleared on
the RX ring only, the handler acks the ISR and calls `wake_any`, which ends
the `poll` `netd` sleeps in on `/dev/net0/data` and its netfs channel (the
kernel's poll reads the wait sequence before it asks the module's readiness
hook, so a frame that lands between the check and the sleep is not missed).
The NIC's `ctl` says `irq on` when that works; on `irq off` (a poll-mode
device) `netd` keeps its 2–10 ms timed polling. Reads of `/dev/netN/data`
stay non-blocking.

## AML opcode set (custom interpreter — not ACPICA)

Used to resolve `Name (_S5_, Package …)` in the DSDT for `/proc/acpi/s5`:

| Opcode | Encoding | Role |
|--------|----------|------|
| Zero / One / Ones | `0x00` / `0x01` / `0xFF` | Integer constants |
| BytePrefix | `0x0A` | `u8` |
| WordPrefix | `0x0B` | `u16` LE |
| DWordPrefix | `0x0C` | `u32` LE |
| QWordPrefix | `0x0E` | `u64` LE |
| StringPrefix | `0x0D` | skipped when scanning |
| NameOp | `0x08` | locate `_S5_` |
| PackageOp | `0x12` | read `slp_typa` / `slp_typb` |
| ScopeOp / MethodOp | `0x10` / `0x14` | scan past |
| ExtOp + DeviceOp | `0x5B 0x82` | scan past |
| ReturnOp | `0xA4` | unwrap integer |
| NameString | Root `\`, `^`, Dual/Multi/NameSeg | path match ending `_S5_` |

PkgLength encoding is implemented. BufferOp / Field / OpRegion / control flow
beyond the above are **not** executed — scan-only.

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
spinning on `gettimeofday`.

`SYS_POLL` (`poll(fds, nfds, timeout_ms)`, `sys_poll`) scans a `struct
pollfd` array with `task::fd_poll` and sleeps on `WAIT_ANY` until the first
fd is ready, the timeout, or a signal. Readiness comes from each object:
pipes and ptys (data, room, the peer gone), the console tty (committed
input; its keyboard is polled, so a watched tty re-checks every 10 ms when
one is present), regular files (always ready), and module files through
the optional `ModuleVfsOps::poll` hook (ABI 17): netfs reports a socket's
bytes, hangup, finished connect, queued accept and, for `/net/unix`, room
in the peer's buffer; a hook that adds `MYOS_POLL_RECHECK` is re-checked
every 10 ms like the tty (`/dev/console/kbd`, a polled device, ABI 20),
and a module `read` that returns `MYOS_READ_WAIT` makes an fd read wait for
the file the same way. libgloss's `poll`/`select` are one call, and so is
every blocking wait of its socket library (`pollselect.c`, `socket.c`).

Deadlines use `time::monotonic_ns()` (x86: TSC calibrated against PIT
channel 2 at boot; aarch64: `CNTVCT_EL0`; riscv64: `time` CSR). Each timer
IRQ calls `task::timer_tick()`, which only scans `TASKS` once the earliest
deadline (`NEXT_DEADLINE`) has passed. The x86 LAPIC tick is calibrated
against that clock to 1 kHz (it used to fire 10-20k times per second per
CPU, each tick taking the scheduler lock). aarch64/riscv64 tick at 100 Hz
but are tickless for deadlines: `block_until` calls
`arch::timer_deadline`, which pulls the CPU's own timer (`cntv_cval_el0` /
`stimecmp`, absolute compare values) in to the new deadline, and every
re-arm programs `min(next tick, task::next_deadline_ns())`, so a 1 ms
`nanosleep` ends after about 1 ms instead of at the next 10 ms boundary.

Idle: `kernel_main` (task 0) is the BSP's idle task, `ap_idle_body` the APs'.
`idle_step` runs whatever is Ready for the CPU, then halts
(`arch::idle_wait`: `sti; hlt` / `wfi` with the pending-interrupt wakeup)
unless `NEED_RESCHED[cpu]` was set meanwhile. A task that blocks with
nothing else runnable halts the same way on its own stack; `schedule` never
picks a woken task that is still some CPU's `CURRENT`, and a wake that lands
while the task is mid-switch (`SWITCHED_FROM`) is deferred to
`finish_switch` (`wake_pending`). `wake` marks the woken task's home CPU
(or the CPU it is halting on, or any idle CPU for a floating task) and sends
a **targeted** reschedule IPI (`arch::ipi_reschedule_cpu`: xAPIC ICR with
a destination, GICv2 SGI with a CPUTargetList bit, SBI IPI with a single
hart) only when that CPU is halted. The console reader re-polls keyboards
every 10 ms (they have no IRQ); the BSP timer stages UART RX on all three
arches and wakes `KEY_CONSOLE`.

The UARTs are programmed once (`SerialPort::new` used to reprogram COM1 /
the PL011 on every output byte; the FIFO-reset bits in that sequence dropped
input that arrived while the kernel echoed, which only showed once the drain
tick slowed down).

`/proc/cpuinfo` reports `schedules`, `idle_halts` per CPU, `blocked_tasks`,
`clock_hz` and `uptime_ms` — an idle shell should show `idle_halts` climbing
and `schedules` nearly flat.

## SMP model per architecture

All three arches use **Limine `MpRequest`**: the bootloader parks APs until
`MpInfo::bootstrap(ap_entry, logical_cpu)`. APs stay online into userspace.

| Arch | CPU id | AP init | Timer / IRQ | IPI |
|------|--------|---------|-------------|-----|
| x86_64 | TSC_AUX / APIC id | Per-CPU GDT+TSS, GS → syscall state, xAPIC timer | LVT timer → `schedule` | xAPIC ICR all-excl-self (vec 33 TLB, 34 resched) |
| aarch64 | `TPIDR_EL1` / `MPIDR_EL1` | naked `goto_address` entry, TTBR0 device map sync, `VBAR`/`use_spx`, banked GICC, timers | PPI timer → `schedule` | GICv2 SGI 0 (TLB), SGI 1 (resched) |
| riscv64 | `tp` / Limine `hartid` | `stvec` / `sie` (STIE+SSIE) / `stimecmp` | S-mode timer → `schedule` | SBI IPI ext → SSIP; soft reason bits in `smp` |

Scheduler: global ready list + optional `affinity` (AP idle threads are pinned).
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
under `-smp 4` TCG (600s wall); do not paper with a longer QEMU timeout. True `affinity: None` live migration
remains off (NX #PF / leave races). `schedule` switches aspace/rsp0 before
publishing Ready (old stays Running across the CR3 write, lock not held during
switch); `unload_user_aspace` briefly kicks remotes then TLB-shootdowns; fork
kicks the child's home CPU only when a child was RR-homed onto another AP.
**aarch64** user tasks were BSP-pinned while block I/O waited on BSP-targeted
SPIs; virtio-blk/NVMe are polled now, so they take AP homes like x86 (the
`tlbi …is` flushes are inner-shareable broadcasts anyway). **riscv64** APs
join the scheduler too (`RISCV_PARK_APS = false`): the U-mode trap entry
reloads `tp` from the CPU-id footer the scheduler stamps at the base of the
task's kernel stack, so `cpu_id()` is right on every hart even though user
TLS lives in `tp`; the `sepc=0` corruption that once forced the WFI park was
the trap vector's t0 clobber (fixed in #179). The aarch64/riscv64 syscall
frame pointer used by fork/exec is per CPU (`SYSCALL_FRAMES`).
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
  APs take a naked entry (FPEN + progress flag) then sync BSP TTBR0 before GIC.
- **riscv64** — `sscratch` holds the current task's kernel stack top (updated on
  every schedule). Per-hart cells are deferred until multi-hart Limine bring-up
  is reliable on QEMU.

TLB shootdown: local invalidate, then IPI the other online CPUs and wait
briefly for acks (`smp::tlb_shootdown`, try-lock + bounded spin). A targeted
reschedule IPI wakes a halted CPU when a task homed there becomes Ready
(`spawn`, `fork`, `wake`).

AP bring-up: IRQs stay masked and `ONLINE` is clear until `ap_idle_loop`
installs `CURRENT` and migrates onto the AP's own 64KiB idle stack (Limine
AP stacks are too small for nested timer/IPI frames).

## `/proc` nodes

- `/proc/mounts` — existing
- `/proc/cpuinfo` — online CPUs, hw ids, schedule counts
- `/proc/pci` — full BDF list from `pci_enum` (hex IDs + class/subclass names and a small QEMU/virt device table). Write `rescan` to re-enumerate and refresh the node (gone devices disappear); the kernel then calls every module's `module_rescan`, and the block drivers (virtio-blk, NVMe) bring up the disks that appeared since boot, leaving the known ones alone. virtio-net probes once at load (netd binds the one `/dev/net0`). No ACPI/QEMU hotplug IRQ yet: a hot-added disk shows up after a rescan.
- `/proc/acpi/info`, `tables`, `s5` — from `acpi` module (honest stubs if no RSDP)

## Out of scope

ACPICA, full OSPM/sleep, Wayland, guest rustc.
