# PCI discovery, ACPI (custom AML), and SMP scheduler

## Module / kernel split

| Component | Location | Why |
|-----------|----------|-----|
| PCI config access (`cfg_read32` / BAR map) | `kernel/src/pci.rs` + `arch/*/pci.rs` | Needed early for NVMe / virtio; module ABI already exposes it |
| Full PCI enumeration → `/proc/pci` | `modules/pci_enum` (`.ko`) | Pure discovery; talks only through `KernelApi` |
| ACPI tables + AML → `/proc/acpi/*` | `modules/acpi` (`.ko`) | Optional; stubs when no RSDP |
| Limine RSDP / MP requests | `kernel/src/limine_boot.rs` | Boot protocol |
| AP bring-up + `/proc/cpuinfo` | `kernel/src/smp.rs` | Must run before modules; owns CPU-local state + IPI helpers |
| Cross-CPU scheduler | `kernel/src/task/` | Per-CPU `CURRENT`, task `affinity`, shared ready set |
| Proc exporters | `kernel/src/fs/procfs.rs` | Built-ins: `mounts`, `cpuinfo`; dynamic via `proc_register` ABI |

ABI version: **9** (`proc_register`, `acpi_rsdp`, `hhdm_offset` appended).

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

## SMP model per architecture

All three arches use **Limine `MpRequest`**: the bootloader parks APs until
`MpInfo::bootstrap(ap_entry, logical_cpu)`. APs stay online into userspace.

| Arch | CPU id | AP init | Timer / IRQ | IPI |
|------|--------|---------|-------------|-----|
| x86_64 | TSC_AUX / APIC id | Per-CPU GDT+TSS, GS → syscall state, xAPIC timer | LVT timer → `schedule` | xAPIC ICR all-excl-self (vec 33 TLB, 34 resched) |
| aarch64 | `TPIDR_EL1` / `MPIDR_EL1` | `VBAR`, `use_spx`, banked GICC, timers; APs still Limine-parked on QEMU virt+UEFI | PPI timer → `schedule` | GICv2 SGI 0 (TLB), SGI 1 (resched) |
| riscv64 | `tp` / Limine `hartid` | `stvec` / `sie` (STIE+SSIE) / `stimecmp` | S-mode timer → `schedule` | SBI IPI ext → SSIP; soft reason bits in `smp` |

Scheduler: global ready list + optional `affinity` (AP idle threads are pinned).
Kernel tasks use `affinity: None` (smoke `sched mask=0x3`). On **x86_64** with
more than one CPU online, new user tasks round-robin across APs (skip BSP); fork
children inherit the parent's affinity until **exec**. After a successful exec,
`replace_user` always re-homes the new image with the same AP-only RR policy
(no basename sticky/rehome allowlists) so `make -j` workers and session
binaries alike spread under `-smp 4` (≥2 APs). Cross-CPU exit reclaim is kept
safe by: (1) `die` enabling IRQs before reclaim/TLB shootdown, (2) epoch-based
TLB shootdown with soft `tlb_service` from `schedule` while IF-off (IRQ-only
ACK previously deadlocked a cli waiter). True `affinity: None` live migration
remains off (NX #PF / leave races). `schedule` switches aspace/rsp0 before
publishing Ready (old stays Running across the CR3 write, lock not held during
switch); `unload_user_aspace` briefly kicks remotes then TLB-shootdowns; fork
kicks idle CPUs. **aarch64** / **riscv64** leave user floating (APs may stay
parked). `note_schedule` → `/proc/cpuinfo`. QEMU `-smp 4` on x86/aarch64
(interactive + CI); riscv stays `-smp 2` (Limine panics `missing struct
riscv_hart for BSP` at 4). The packed `boot/virt.dtb` is always dumped with
`-smp 2` so OpenSBI BSP hartid=1 still has a hart node. riscv APs are
WFI-parked in-kernel without ONLINE (Limine busy-spin left the ripgrep
`sepc=0` IPF; full ONLINE bring-up hung mid-`/ok`). On x86 the BSP timer also
drains UART RX (`input::drain_uart_irq`) and kicks APs so a starved shell
under `-smp 4` TCG cannot overrun the COM1 FIFO / sticky-key login.

Per-CPU ring3↔ring0 state:

- **x86_64** — each CPU has its own GDT+TSS (`rsp0` / DF IST). `IA32_EFER.NXE`
  and `IA32_GS_BASE` → `CpuSyscallState` are programmed on BSP and every AP
  (`kernel_rsp0` + fork callee snapshot). User CS/SS come from the CPU's GDT.
- **aarch64** — banked `SP_ELx` after `use_spx`; exception frames live on the
  current task's kernel stack. Limine `goto_address` handoff on QEMU virt+UEFI
  still does not enter the kernel AP stub; APs stay parked (SGI paths ready).
- **riscv64** — `sscratch` holds the current task's kernel stack top (updated on
  every schedule). Per-hart cells are deferred until multi-hart Limine bring-up
  is reliable on QEMU.

TLB shootdown: local invalidate, then IPI the other online CPUs and wait
briefly for acks (`smp::tlb_shootdown`, try-lock + bounded spin). Reschedule
IPI wakes idle CPUs after `spawn` when `online_count() > 1`.

AP bring-up: IRQs stay masked and `ONLINE` is clear until `ap_idle_loop`
installs `CURRENT` and migrates onto the AP's own 64KiB idle stack (Limine
AP stacks are too small for nested timer/IPI frames).

## `/proc` nodes

- `/proc/mounts` — existing
- `/proc/cpuinfo` — online CPUs, hw ids, schedule counts
- `/proc/pci` — full BDF list from `pci_enum`
- `/proc/acpi/info`, `tables`, `s5` — from `acpi` module (honest stubs if no RSDP)

## Out of scope

ACPICA, full OSPM/sleep, Wayland, guest rustc.
