# PCI discovery, ACPI (custom AML), and SMP scheduler

## Module / kernel split

| Component | Location | Why |
|-----------|----------|-----|
| PCI config access (`cfg_read32` / BAR map / `pci_find` / `pci_find_class`) | `kernel/src/pci.rs` + `arch/*/pci.rs` | The module ABI exposes it; every PCI driver is a module |
| virtio-blk, NVMe | `modules/virtio_blk`, `modules/nvme` | Register `/dev/<name>` through `blk_register`; loaded before the filesystem modules |
| USB: xHCI host, hub and mass storage | `modules/xhci`, `modules/usb_hub`, `modules/usb_storage` | The host publishes the USB bus as a service the class drivers look up; sticks are `/dev/sdX/`, hot-pluggable (`docs/usb.md`) |
| Framebuffer text, keyboards, keymap | `modules/console` | Serial stays in the kernel; the module registers screen + keyboard ops (`console_register`) |
| Linux syscall layer | `modules/linux` | Registers a syscall *personality* (`personality_register`); the kernel keeps only which tasks have it (`kernel/src/personality.rs`) |
| Full PCI enumeration → `/proc/pci` | `modules/pci_enum` (`.ko`) | Discovery + on-demand rescan via write; talks only through `KernelApi` |
| Platform description (ACPI static tables, device tree) | `kernel/src/platform.rs`, `acpi.rs`, `dt.rs` | Device bases, interrupt routing and clocks come from it, before the heap exists |
| ACPI tables + AML → `/proc/acpi/*` | `modules/acpi` (`.ko`) | Optional; stubs when no RSDP |
| Limine RSDP / MP requests | `kernel/src/limine_boot.rs` | Boot protocol |
| AP bring-up + `/proc/cpuinfo` | `kernel/src/smp.rs` | Must run before modules; owns CPU-local state + IPI helpers |
| Cross-CPU scheduler | `kernel/src/task/` | Per-CPU `CURRENT`, task `affinity`, shared ready set |
| Proc exporters | `kernel/src/fs/procfs.rs` | Built-ins: `mounts`, `cpuinfo`, `platform`; dynamic via `proc_register` ABI |

ABI version: **21** (`blk_unregister`, `service_register` / `service_lookup`, `thread_spawn`, `wake` and the USB bus types, `docs/usb.md`; see `README.md` for 15–20; 14: `dt_mmio_find`: a module finds its memory-mapped devices in the device tree; 13: `personality_register`, `personality_exec`, `native_syscall` and the task / fd / VFS / signal / FPU / wait helpers a syscall personality needs: the Linux layer is a module; 12: `blk_register`, `pci_find_class`, `framebuffer_info`, `console_register`: block devices and the console are modules; 11: `pci_irq_enable`, `wake_any`, `wait_seq`, `block_until`, `monotonic_ns` for device interrupts and blocking waits; 10: `proc_set_writer` for `/proc/pci` rescan; earlier: `proc_register`, `acpi_rsdp`, `hhdm_offset`).

Boot modules come from the initramfs: `/lib/modules/<name>` for each name in `/lib/modules/boot.list` (`src/limine_image.rs` `BOOT_MODULES` plus the `OPTIONAL_MODULES` whose Cargo feature is on, in load order); `insmod <path>` (`SYS_INSMOD`) loads more at runtime and `/proc/modules` lists them.

## Device interrupts

`kernel/src/irq.rs` keeps one handler per interrupt number and `pci_irq_setup`
picks the delivery mechanism per arch, so a module only asks for "this PCI
function's interrupt" (`KernelApi::pci_irq_enable`) and gets told whether to
use an MSI-X table entry or the INTx line:

| Arch | Mechanism | Numbering |
|------|-----------|-----------|
| x86_64 | MSI-X entry 0 → LAPIC vector on the BSP (`DEVICE_VECTOR_BASE` 48 + n, 16 vectors shared with the I/O APIC pins below; no PIRQ routing; a BSP with an x2APIC id above 255 would need interrupt remapping, so it gets no MSI) | vector |
| aarch64 | INTx → GIC SPI, level, priority 0x80, delivered to the BSP (GICv2: `ITARGETSR` CPU 0; GICv3: `IROUTER` to its affinity); the SPI comes from the device tree's PCIe `interrupt-map` (QEMU `virt`: SPI 3 + (slot + pin − 1) mod 4) | INTID (35..38) |
| riscv64 | INTx → PLIC source for the boot hart's S-mode context, from the same `interrupt-map` (`virt`: 32 + (slot + pin − 1) mod 4), `sie.SEIE` | PLIC source |

A board device's own interrupt goes through `irq::enable` (modules:
`KernelApi::irq_enable`, ABI 33), which registers the handler before it
unmasks the line. On aarch64 and riscv64 the number is the one the device
tree or the SPCR gives (a GIC SPI, a PLIC source; `MmioDevice::irq` for a
module). On x86_64 it is a legacy ISA IRQ: `arch/x86/ioapic.rs` drives the
MADT's first I/O APIC, takes the GSI and polarity/trigger from the
interrupt source overrides, and points the pin at a fresh device vector
on the BSP (the 8259s stay masked). The console's input uses it: the UART's
receive interrupt (COM1's IRQ 4, the SPCR's or device tree's on the
others), the PS/2 keyboard's IRQ 1 and the virtio-input device's. A PC
without an MADT I/O APIC routes nothing: the UART is then drained by CPU
0's tick and the keyboard polled, as before.

## The platform description

Nothing about the board is hard-coded: `kernel/src/platform.rs` describes
it once at boot, before the heap and the first console output, and the
arch code takes its device bases from that description and from nowhere
else (`arch::apply_platform`). Two sources fill it, in this order:

1. **The ACPI static tables** (`kernel/src/acpi.rs`), when the firmware
   hands Limine an RSDP and the arch prefers them (`arch::PREFER_ACPI`:
   x86_64 and aarch64; not riscv64, whose ACPI tables are younger than
   the firmware that runs here). Only tables that name the platform are
   read, and no AML: the MADT (CPUs; the local APIC, or the GICD, GICC,
   GICR and ITS entries), the MCFG (PCIe ECAM and bus range), the SPCR
   (the console UART and its interrupt), the GTDT (the arm timer
   interrupts) and the FADT's arm boot flags (PSCI). Allocation-free,
   read in place through the HHDM. `modules/acpi` keeps `/proc/acpi` and
   the ACPI power methods (`_S5` power-off, the reset register:
   `docs/power.md`).
2. **The device tree** (`kernel/src/dt.rs`, the `fdt` crate, MPL-2.0),
   when Limine hands one over. It fills what nothing described yet and is
   compared against what the tables did: a component both describe
   differently is flagged (`[WARN] platform: acpi and dt differ on ...`
   at boot, `differs` in `/proc/platform`) rather than silently resolved
   in favour of either.

| Component | ACPI | Device tree (`compatible`) |
|------|------|------|
| CPUs | MADT enabled LAPIC / x2APIC / GICC entries | `/cpus` children |
| Interrupt controller | MADT: LAPIC address; GICD (with the GIC version) + GICC → GICv2, GICD + GICR → GICv3 | `arm,cortex-a15-gic` / `arm,gic-400` (GICv2), `arm,gic-v3`, `sifive,plic-1.0.0` / `riscv,plic0` |
| Console UART | SPCR: interface type (16550 or PL011 / SBSA), address, GSIV | `arm,pl011`, `ns16550a` + `interrupts` |
| RTC | — | `arm,pl031`, `google,goldfish-rtc` (optional: no RTC, no wall clock) |
| Timers | GTDT: EL1 physical and virtual timer INTIDs | `/cpus` `timebase-frequency` (riscv64 `time` CSR rate) |
| PCIe host bridge | MCFG: ECAM base, bus range | `pci-host-ecam-generic`: `reg`, `bus-range` |
| PCIe MMIO windows, INTx routing | — (in AML) | `ranges` (the 32-bit entry on aarch64, the 64-bit one on riscv64), `interrupt-map` / `interrupt-map-mask` |
| virtio-mmio transports | — | `virtio,mmio`, for modules through `KernelApi::dt_mmio_find`, in ascending address order |
| PSCI (aarch64 power-off and reset, `docs/power.md`) | FADT arm boot flags: PSCI compliant, HVC or SMC | `arm,psci-1.0` / `arm,psci-0.2` / `arm,psci`: `method` |

What each arch requires of the description: aarch64 a GIC (v2: the
distributor and the CPU interface; v3: the distributor and the
redistributor region, below), a PL011 or a 16550 (with its register stride
and access width: the tree's `reg-shift` / `reg-io-width`, the SPCR's
access size) and a PCIe host bridge with a 32-bit MMIO window; riscv64 a PLIC, a
16550, the timebase and a host bridge with a 64-bit window; x86_64
takes the MCFG's ECAM window for PCI configuration space when the
description has one (QEMU `q35`, every PCIe PC; `arch/x86/pci.rs` maps a
bus's megabyte of it on first use, once the heap's frames exist) and
needs nothing else (the LAPIC base comes from its MSR, the console from
COM1, configuration space from port 0xCF8 without an MCFG), so a PC
without ACPI tables boots too (QEMU's `pc` machine publishes neither an
SPCR nor an MCFG: its description is the CPUs and the LAPIC).
`MYOS_X86_MACHINE=q35 cargo run -- test-mini` boots the PCIe PC locally;
CI boots `pc`. A missing source or component stops the boot with `fatal: platform:
...` on the UART at QEMU `virt`'s address (the one assumption left, so
the message has somewhere to go).

`/proc/platform` shows the description, one line per component, each
ending in the source it came from:

```
source acpi+dt
model linux,dummy-virt (dt)
oem BOCHS (acpi)
cpus 4 (acpi)
intc gicv2 0x8000000 0x8010000 (acpi)
uart pl011 0x9000000 irq 33 (acpi)
rtc pl031 0x9010000 (dt)
timer irq 30 27 (acpi)
pci ecam 0x3f000000 0x1000000 bus 0-15 (acpi)
pci mmio32 0x10000000 0x2eff0000 (dt)
```

The kernel command line (`cmdline:` in `limine.conf`) forces one source
when a board's firmware gets the other wrong: `platform=acpi` ignores the
tree, `platform=dt` the tables (on riscv64 it also makes the kernel read
tables it would otherwise skip: `platform=acpi` there is the opt-in).

**GICv3** (`arch/aarch64/interrupts.rs`): the distributor runs with
affinity routing (every SPI in group 1, routed to the BSP's MPIDR
affinity by `GICD_IROUTER` when a driver enables it); each CPU finds its
redistributor by walking the region's frames (128 KiB each, 256 KiB with
the GICv4 VLPI frames) for the `GICR_TYPER` naming its affinity, wakes
it, enables its SGIs and timer PPIs there, and drives the CPU interface
through the `ICC_*` system registers (`ICC_SRE_EL2` as well when the
kernel runs at EL2; `ICC_IAR1`/`ICC_EOIR1` in the handler, `ICC_SGI1R`
for the IPIs with the target's affinity). The boot log says which GIC
(`[INFO] gicv3`). QEMU `virt` gives a GICv3 with `gic-version=3`:
`MYOS_AARCH64_GIC=3 cargo run -- aarch64 test-mini` (the launcher dumps
the matching tree); CI boots both versions (`aarch64`, `aarch64-gicv3`).
The ITS (message-signalled interrupts) is not driven: PCI devices use
INTx on aarch64.

The EDK2 firmware boots (AAVMF, RISC-V EDK2) hand Limine no tree, so the
host tool dumps QEMU's (`-machine ...,dumpdtb`, same machine options and
`-smp` as the boot) into the ESP as `boot/virt-aarch64.dtb` /
`boot/virt.dtb` and `limine.conf` passes it with `global_dtb`; AAVMF also
publishes ACPI tables, so the aarch64 boots exercise the agreement check
(`source acpi+dt`). Interrupt specifiers of the tree are decoded per arch
(`arch::irq_from_dt`: GIC `<type number flags>` → INTID, PLIC `<source>`).

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

Used to resolve `Name (_S5_, Package …)` in the DSDT for `/proc/acpi/s5`
and the ACPI power-off (`docs/power.md`). The name is found by its bytes:
the segment `_S5_` right after a NameOp (`0x08`, the root prefix `\` may
come between) and followed by a PackageOp. The opcodes are not walked:
staying in step would take every one of them (a `0x08` in a buffer is no
NameOp).

| Opcode | Encoding | Role |
|--------|----------|------|
| Zero / One / Ones | `0x00` / `0x01` / `0xFF` | Integer constants |
| BytePrefix | `0x0A` | `u8` |
| WordPrefix | `0x0B` | `u16` LE |
| DWordPrefix | `0x0C` | `u32` LE |
| QWordPrefix | `0x0E` | `u64` LE |
| NameOp | `0x08` | before `_S5_` |
| PackageOp | `0x12` | read `slp_typa` / `slp_typb` |
| ReturnOp | `0xA4` | unwrap integer |

PkgLength encoding is implemented. Nothing is executed.

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

Idle: `kernel_main` (task 0) is the BSP's idle task, `ap_idle_body` the APs'.
`idle_step` runs whatever is Ready for the CPU, then halts
(`arch::idle_wait`: `sti; hlt` / `wfi` with the pending-interrupt wakeup)
unless `NEED_RESCHED[cpu]` was set meanwhile. A task that blocks with
nothing else runnable halts the same way on its own stack; `schedule` never
picks a woken task that is still some CPU's `CURRENT`, and a wake that lands
while the task is mid-switch (`SWITCHED_FROM`) is deferred to
`finish_switch` (`wake_pending`). A task leaving a CPU stays Running until
`finish_switch`, also one a wake already made Ready while it halted there:
left Ready, a peer could resume it from its previous, stale switch frame
before `task_switch` saved the new one. `wake` marks the woken task's home CPU
(or the CPU it is halting on, or any idle CPU for a floating task) and sends
a **targeted** reschedule IPI (`arch::ipi_reschedule_cpu`: the LAPIC ICR with
a destination, a GIC SGI to one CPU (v2: a CPUTargetList bit, v3: the affinity in `ICC_SGI1R_EL1`), SBI IPI with a single
hart) only when that CPU is halted. The UART's receive interrupt stages its
bytes (`input::drain_uart_irq`, into a lock-free ring) and wakes
`KEY_CONSOLE`; the keyboard's only wakes it (`KernelApi::console_input`),
and the reader takes the keys. Without those interrupts the BSP's tick
stages UART RX and the console reader re-polls the keyboard every 10 ms.

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
| x86_64 | TSC_AUX / APIC id | Per-CPU GDT+TSS, GS → syscall state, LAPIC timer (x2APIC MSRs when the CPU has an x2APIC, the xAPIC page otherwise; QEMU's TCG has no x2APIC, so CI runs the xAPIC path) | LVT timer → `schedule` | ICR all-excl-self (vec 33 TLB, 34 resched) |
| aarch64 | `TPIDR_EL1` / `MPIDR_EL1` | naked `goto_address` entry, TTBR0 device map sync, `VBAR`/`use_spx`, the CPU's GIC bank (v2: banked GICC; v3: its redistributor + `ICC_*`), timers | PPI timer → `schedule` | GICv2 SGI 0 (TLB), SGI 1 (resched) |
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
- `/proc/platform` — the board description and where each component came from (above)
- `/proc/usb` — the controllers (ports, interrupt, event counts) and the USB devices: id, parent hub and port, speed, vendor:product, each interface's class triple and the driver that took it (`docs/usb.md`)
- `/proc/pci` — full BDF list from `pci_enum` (hex IDs + class/subclass names and a small QEMU/virt device table). Write `rescan` to re-enumerate and refresh the node (gone devices disappear); the kernel then calls every module's `module_rescan`, and the block drivers (virtio-blk, NVMe) bring up the disks that appeared since boot, leaving the known ones alone. virtio-net probes once at load (netd binds the one `/dev/net0`). No ACPI/QEMU hotplug IRQ yet: a hot-added disk shows up after a rescan.
- `/proc/acpi/info`, `tables`, `s5` — from `acpi` module (honest stubs if no RSDP)

## Out of scope

ACPICA, full OSPM/sleep, Wayland, guest rustc.
