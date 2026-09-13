# PCI discovery, ACPI (custom AML), and SMP scheduler

## Module / kernel split

| Component | Location | Why |
|-----------|----------|-----|
| PCI config access (`cfg_read32` / BAR map) | `kernel/src/pci.rs` + `arch/*/pci.rs` | Needed early for NVMe / virtio; module ABI already exposes it |
| Full PCI enumeration → `/proc/pci` | `modules/pci_enum` (`.ko`) | Pure discovery; talks only through `KernelApi` |
| ACPI tables + AML → `/proc/acpi/*` | `modules/acpi` (`.ko`) | Optional; stubs when no RSDP |
| Limine RSDP / MP requests | `kernel/src/limine_boot.rs` | Boot protocol |
| AP bring-up + `/proc/cpuinfo` | `kernel/src/smp.rs` | Must run before modules; owns CPU-local state |
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
`MpInfo::bootstrap(ap_entry, logical_cpu)`.

| Arch | CPU id | AP init | Timer / IRQ |
|------|--------|---------|-------------|
| x86_64 | CPUID.1 APIC id | Load GDT/IDT, enable this CPU's xAPIC timer | Existing LVT timer → `schedule` |
| aarch64 | `MPIDR_EL1` | `VBAR`, banked GICC, generic timers | PPI timer → `schedule` |
| riscv64 | Limine `hartid` (logical in `tp`) | `stvec` / `sie` / `stimecmp` | S-mode timer → `schedule` |

Scheduler: global ready list + optional `affinity` (AP idle threads are pinned).
Any CPU may run `affinity: None` tasks; `note_schedule` counts per-CPU ticks
exposed in `/proc/cpuinfo`. QEMU launches use `-smp 2`.

## `/proc` nodes

- `/proc/mounts` — existing
- `/proc/cpuinfo` — online CPUs, hw ids, schedule counts
- `/proc/pci` — full BDF list from `pci_enum`
- `/proc/acpi/info`, `tables`, `s5` — from `acpi` module (honest stubs if no RSDP)

## Out of scope

ACPICA, full OSPM/sleep, IPI TLB shootdown, per-CPU TSS/GDT, Wayland, guest rustc.
