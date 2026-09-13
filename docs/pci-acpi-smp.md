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
| aarch64 | `TPIDR_EL1` / `MPIDR_EL1` | `VBAR`, `use_spx`, banked GICC, timers | PPI timer → `schedule` | GICv2 SGI 0 (TLB), SGI 1 (resched) |
| riscv64 | `tp` / Limine `hartid` | `stvec` / `sie` (STIE+SSIE) / `stimecmp` | S-mode timer → `schedule` | SBI IPI ext → SSIP; soft reason bits in `smp` |

Scheduler: global ready list + optional `affinity` (AP idle threads are pinned).
User and kernel tasks use `affinity: None` and may run on any online CPU.
`note_schedule` counts per-CPU ticks exposed in `/proc/cpuinfo`. QEMU launches
use `-smp 2`.

Per-CPU ring3↔ring0 state:

- **x86_64** — each CPU has its own GDT+TSS (`rsp0` / DF IST). `IA32_GS_BASE`
  points at that CPU's `CpuSyscallState` (`kernel_rsp0` + fork callee snapshot).
- **aarch64** — banked `SP_ELx` after `use_spx`; exception frames live on the
  current task's kernel stack.
- **riscv64** — per-CPU `sscratch` kernel stack top updated on every schedule.

TLB shootdown: local invalidate, then IPI the other online CPUs and wait for
acks (`smp::tlb_shootdown`). Reschedule IPI wakes idle CPUs after `spawn`.

## `/proc` nodes

- `/proc/mounts` — existing
- `/proc/cpuinfo` — online CPUs, hw ids, schedule counts
- `/proc/pci` — full BDF list from `pci_enum`
- `/proc/acpi/info`, `tables`, `s5` — from `acpi` module (honest stubs if no RSDP)

## Out of scope

ACPICA, full OSPM/sleep, Wayland, guest rustc.
