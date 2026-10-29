# The IOMMU

A PCI device reads and writes memory at the addresses its driver gives
it. A program driving one through `/dev/pci` (`docs/pci.md`) gives those
addresses itself, so a bug in it, or in the device's own firmware, could
reach kernel memory or another process's pages. The IOMMU is the
platform's unit between the devices and memory. The kernel
(`kernel/src/iommu`, issue #501) programs it to confine such a device to
its own buffers and its own interrupt.

## Domains

- **Kernel drivers: passthrough.** A device a kernel module drives stays
  in the *identity domain*, which maps all of memory one-to-one. The
  drivers need nothing from the IOMMU.
- **Programs: a domain of their own.** A function a program holds through
  `/dev/pci` gets its own domain at the `ctl` open
  (`KernelApi::iommu_attach`). The domain holds only the DMA buffers the
  program asked for (`dma PAGES`) and the ones kept from the last driver.
  Each buffer sits at its physical address, so the numbers `ctl` shows
  stay the device's. Closing `ctl` puts the function back in passthrough.
- **Everything else is refused.** The unit refuses a DMA anywhere else
  and records it; its fault interrupt reads the record.

`/proc/iommu` lists the units, the domains with what they map, `faults
N` and the last fault (`last BB:SS.F read|write ADDRESS reason R`). On a
board without an IOMMU it says `none`. The `pcidev_smoke` test
(`docs/pci.md`) checks the confinement of DMA and of interrupts.

## Interrupts

A device's interrupts (MSIs) are memory writes too. How a confined
function is kept to its own interrupt depends on the arch.

- **x86_64: interrupt remapping.** Each message a function is given
  names an entry of the unit's interrupt remapping table, which holds the
  vector and answers to that function only. `iommu/x86.rs` knows the two
  units' formats; `arch/x86/interrupts.rs` and `ioapic.rs` program the
  messages and the I/O APIC pins through it. VT-d refuses plain-format
  messages once its table is in place, and records a refused message
  like a DMA; AMD-Vi aborts the interrupts of a device without a table
  and logs nothing in QEMU. `/proc/iommu` shows the entries in use
  (`intremap N/256`).
- **aarch64: the doorbell mapped.** The MSI controller's doorbell page
  (`docs/interrupts.md` "MSI controllers") is mapped into the domain.
  The ITS tells devices apart by requester id. A GICv2m frame cannot, so
  a confined function could raise another device's SPI there; handlers
  take that as a spurious interrupt (issue #590).
- **riscv64: memory-resident interrupt files.** Where the unit has them
  (`CAP.MSI_MRIF`, `mrif` in `/proc/iommu`), the function's writes to the
  IMSIC page land in an MRIF of its own with one identity enabled, and
  the unit notifies the kernel, which raises the function's interrupt.
  Whatever the function writes, it raises that identity or nothing. A
  unit without MRIFs gets the IMSIC page mapped as on aarch64.

A remapped level-triggered pin is also ended at the I/O APIC's EOI
register (`ioapic::eoi`): QEMU's AMD unit delivers every remapped
interrupt as edge-triggered, so the LAPIC's EOI alone would leave the
line silent after its first interrupt.

QEMU's units let plain-format messages through, which hardware refuses.
`pcidev_smoke` therefore checks that a message rewritten to another
entry or identity is not delivered.

## The units

| Arch | Unit | Code | Described by |
|---|---|---|---|
| x86_64 | Intel VT-d | `iommu/vtd.rs` | the ACPI DMAR table |
| x86_64 | AMD-Vi | `iommu/amdvi.rs` | the ACPI IVRS table |
| aarch64 | Arm SMMUv3 | `iommu/smmuv3.rs` | the IORT, or the tree's `arm,smmu-v3` node and the host's `iommu-map` (`iommu smmuv3 ...` in `/proc/platform`, `docs/platform.md`) |
| riscv64 | the RISC-V IOMMU | `iommu/riscv.rs` | a PCI function of class 08.06, claimed by the kernel so `/dev/pci` cannot hand it out |

Each driver keeps passthrough (or bypass, bare) for the kernel's devices
and a second-stage page table per program domain. Its file describes the
tables, invalidation and fault reporting. The riscv64 kernel reads the
fault queue when `/proc/iommu` is read, since the unit signals faults by
MSI only.

## In QEMU

| Machine | Unit | Boot line (`[ OK ]`) |
|---|---|---|
| x86_64 `q35`, every x86 boot | `-device intel-iommu,intremap=on` | `iommu: vt-d, 1 unit(s), 3-level tables, 1 GiB superpages, memory to 0x..., interrupts remapped` |
| x86_64 `q35` with `MYOS_X86_IOMMU=amd` (CI's `bios-amd`) | `-device amd-iommu,intremap=on,dma-remap=on` | `iommu: amd-vi, 1 unit(s), 256 device ids, 4-level tables, interrupts remapped` |
| aarch64 `virt`, every boot | `iommu=smmuv3` | `iommu: smmuv3 at 0x9050000, 4096 streams, 44-bit stage 2` |
| riscv64 `virt` | `-device riscv-iommu-pci` (`00:01.0`) | `iommu: riscv unit at 00:01.0, 256 device ids, Sv39x4 second stage, MRIFs` |

- **AMD's unit needs QEMU 10.1 or later**: an older QEMU refuses a DMA to
  an unmapped page without logging it. The launcher then keeps Intel's
  unit and says so once.
- **The RISC-V IOMMU needs QEMU 9.2 or later** (Ubuntu 24.04 has 8.2).
  Without it the launcher says so once and boots without.
- **The `pc` machine has no IOMMU** (`MYOS_X86_MACHINE=pc`,
  `docs/platform.md`).
- **The UEFI boots run OVMF from edk2-stable202505** (the launcher's
  `OVMF` constant): edk2-stable202605 faults inside the firmware on a
  `q35` with the IOMMU and an xHCI controller.

A board without an IOMMU prints no boot line, and `/proc/iommu` says
`none`.
