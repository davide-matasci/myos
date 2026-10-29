# The IOMMU

A PCI device reads and writes memory at the addresses its driver gives
it. A program driving one through `/dev/pci` (`docs/pci.md`) gives those
addresses itself, so a bug in it, or in the device's own firmware, could
reach kernel memory or another process's pages. The IOMMU is the
platform's unit between the devices and memory. The kernel
(`kernel/src/iommu`, issue #501) programs it to confine such a device to
its own buffers, and on x86_64 to its own interrupt vector. This page
describes what the kernel does with the unit, the units it drives on each
arch, and the QEMU machines that have one.

## Domains

- **Kernel drivers: passthrough.** A device a kernel module drives stays
  in the *identity domain*, which maps all of memory one-to-one with the
  largest pages the unit has. The drivers need nothing from the IOMMU.
- **Programs: a domain of their own.** A function a program holds through
  `/dev/pci` gets its own domain (`KernelApi::iommu_attach`, ABI 49, at
  the `ctl` open). The domain holds only the DMA buffers the program
  asked for (`iommu_map`, at each `dma PAGES`), and the ones kept from the
  last driver. Each buffer sits at its physical address, so the numbers
  `ctl` shows stay the device's. Closing `ctl` puts the function back in
  passthrough (`iommu_detach`).
- **Everything else is refused.** The unit refuses a DMA anywhere else
  and records it. Its fault event is an interrupt (an MSI to a device
  vector), whose handler reads the fault recording registers.

`/proc/iommu` lists the units, the domains with what they map, `faults
N` and the last fault (`last BB:SS.F read|write ADDRESS reason R`). On a
board without an IOMMU it says `none`.

## Interrupts

A device's interrupts are messages it writes too (MSIs). How the IOMMU
keeps a confined function to its own interrupt depends on the arch.

### Interrupt remapping on x86_64

On x86_64 an MSI is a write to the local APIC's address, and the unit
remaps it. Each message a function is given names an entry of an
interrupt remapping table. The entry holds the vector and answers to that
function only, so a function raises its own vector and no other's.
`iommu/x86.rs` knows both units' message formats; `arch/x86/interrupts.rs`
programs the devices' messages, and `ioapic.rs` the I/O APIC's pins the
same way.

- **VT-d** names the entry in the message's address (the *remappable*
  format). There is one table per unit; `intremap N/256` on the unit's
  `/proc/iommu` line counts the entries given out, and `queued` there
  means it invalidates through its queue. The entry verifies the sender's
  bus, device and function. Once the table is in place the unit refuses a
  plain-format message (`GCMD.CFI` clear).
- **AMD-Vi** names the entry in the message's data (bits 10:0), in a
  table of the device's own (`intremap N table(s)` on its line). A device
  without a table has its interrupts aborted.

A remapped level-triggered pin (a PCI INTx line) is ended at the I/O
APIC's EOI register as well as at the LAPIC (`ioapic::eoi`). The LAPIC's
EOI clears the pin's Remote IRR only for an interrupt delivered as
level-triggered with the vector the entry holds. An AMD-Vi entry holds
its index there, and QEMU's AMD unit delivers every remapped interrupt as
edge-triggered, so without the I/O APIC EOI the line would go silent
after its first interrupt.

VT-d records a refused message like a DMA (reason 0x20 and up, 0x26 for
another function's entry, the address being its handle). QEMU's AMD unit
logs none.

QEMU's units let plain-format messages through, which hardware refuses.
`pcidev_smoke` therefore checks the entry instead: the device's message
rewritten to the next entry is not delivered (and is counted, on VT-d),
and its own message arrives again afterwards.

### MSIs on aarch64 and riscv64

On aarch64 and riscv64 a message is a write to the MSI controller's
doorbell (`docs/interrupts.md` "MSI controllers"): the ITS's
`GITS_TRANSLATER`, the GICv2m frame's `MSI_SETSPI_NS`, the boot hart's
IMSIC page. That is a DMA like any other, which the unit sees when the
function is in a domain of its own (`iommu::msi_message`, when its
interrupt is routed).

- **aarch64**: the doorbell's page is mapped into the domain at its own
  address; the SMMUv3's MSI translation stays unused.
  - The ITS tells the devices apart by their requester id, so a confined
    function raises its own events and no other's.
  - A GICv2m frame takes any SPI of its range it is sent, so a confined
    function could raise another device's MSI there. Those are spurious
    interrupts, which handlers take as such, since they check their
    device's status. The frame has nothing to tell the senders apart, and
    INTx instead would take MSI away from the confined functions (issue
    #590).
- **riscv64**: the RISC-V IOMMU translates the function's messages where
  it has memory-resident interrupt files (`CAP.MSI_MRIF`; `mrif` on its
  `/proc/iommu` line). It uses its device context's MSI page table, flat
  (chapter 2.3 of the specification).
  - The IMSIC page's writes go to an MRIF of the function's own: a
    512-byte page in memory holding a pending and an enable bit per
    identity, with one identity enabled.
  - The unit *notices* a write with a message of its own, which raises
    the function's IMSIC identity. The domain's line says `mrif ID
    pending 0xBITS`, the file's first 64 pending bits.
  - Whatever the function writes, it raises that identity or nothing; the
    IMSIC's page itself is not in the domain.
  - The kernel clears the file's pending bits when it claims the notice,
    before the handlers run, so the next message is noticed again.
  - A unit without MRIFs gets the IMSIC page mapped into the domain as on
    aarch64, and the IMSIC then takes any identity.

  `pcidev_smoke` rewrites the device's message to the console UART's
  identity. It lands as a pending bit of the device's file and nothing
  arrives; the device's own message arrives once it is put back.

A DMA to the interrupt controller's other registers is refused like any
other, which the smoke checks too.

## The units

| Arch | Unit | Where it is described |
|---|---|---|
| x86_64 | Intel VT-d (`iommu/vtd.rs`): a root table per unit (one entry per bus), a context table per bus (one entry per device and function), second-level page tables of the CPU's shape (3 levels for 39-bit addresses, 4 for 48, 2 MiB and 1 GiB superpages where `CAP.SLLPS` has them), invalidation through the queue where the unit has one (`ECAP.QI`; the registers otherwise; whole: the context cache, the IOTLB, the interrupt entry cache), interrupt remapping (`ECAP.IR` and the DMAR's `INTR_REMAP` flag; a table of 256 entries per unit, one per message or I/O APIC pin routed, extended interrupt mode where the unit has it, `ECAP.EIM`, and the BSP runs in x2APIC mode), the tables' cache lines flushed for a unit whose walks do not snoop (`ECAP.C` clear) | the ACPI DMAR table: each DRHD's registers and device scopes (endpoints and bridges), the `INCLUDE_PCI_ALL` one for the rest |
| x86_64 (AMD) | AMD-Vi (`iommu/amdvi.rs`, specification revision 3): a flat device table (a 32-byte entry per device id, for the buses with a function on them), *passthrough* for every function (mode 0: no table walked) and a 4-level page table of AMD's format for one a program holds (a domain id each); the command buffer for invalidations (an entry, a domain's pages, an interrupt table, everything where the unit has `IASup`), each batch ended by a completion wait; the event log for the refused DMAs, its interrupt the MSI of the unit's own PCI function (class 08.06, claimed by the kernel); an interrupt remapping table per device (4-byte entries, no guest mode) | the ACPI IVRS table: each IVHD's registers, own device id and device entries (all, select, range, the I/O APIC as a special device) |
| aarch64 | Arm SMMUv3 (`iommu/smmuv3.rs`): a linear stream table, one entry per requester id, *bypass* for every function (no table walked) and a *stage-2* entry (VMSAv8, 4 KiB granule, a VMID per domain) for one a program holds; the command queue for invalidations and syncs, the event queue (its interrupt, which the SMMU pulses, taken edge-triggered) for the refused DMAs; the tables and queues written back for a unit that does not snoop (`IDR0.COHACC` clear) | the IORT's SMMUv3 node (registers, the event and global error GSIVs) and the root complex node's ID mapping to it, or the tree's `arm,smmu-v3` node and the PCIe host's `iommu-map`: a `platform::Iommu` component, `/proc/platform`'s `iommu smmuv3 ...` line (`docs/platform.md`) |
| riscv64 | the RISC-V IOMMU (`iommu/riscv.rs`, specification 1.0): a three-level device directory, one 64-byte context per device id (32 bytes for a unit without MSI translation), *bare* for every function of a populated bus (no table walked) and a *second-stage* context (a G-stage table in the CPU's Sv39x4 or Sv48x4 format, a GSCID per domain) for one a program holds; the command queue for invalidations and fences, the fault queue for the refused DMAs, read when `/proc/iommu` is (the PCI unit signals it by MSI only, which the riscv64 kernel does not take); MSI translation through memory-resident interrupt files for the confined functions (`CAP.MSI_MRIF`, above) | a PCI function of class 08.06 (system peripheral, IOMMU), its registers in BAR 0, claimed by the kernel so `/dev/pci` cannot hand it out; nothing from the tree |

## In QEMU

| Machine | Unit | Boot line (`[ OK ]`) |
|---|---|---|
| x86_64 `q35`, every x86 boot of the launcher and of CI | Intel's: `-device intel-iommu,intremap=on` (39-bit, 3-level, both superpage sizes, snooping walks, queued invalidation, interrupt remapping) | `iommu: vt-d, 1 unit(s), 3-level tables, 1 GiB superpages, memory to 0x..., interrupts remapped` |
| x86_64 `q35` under `MYOS_X86_IOMMU=amd` (CI's `bios-amd` machine) | AMD's: `-device amd-iommu,intremap=on,dma-remap=on` | `iommu: amd-vi, 1 unit(s), 256 device ids, 4-level tables, interrupts remapped` |
| aarch64 `virt`, every boot | an SMMUv3 (`iommu=smmuv3`: 44-bit output, snooping, SPIs 74 to 77 for its queues) | `iommu: smmuv3 at 0x9050000, 4096 streams, 44-bit stage 2` |
| riscv64 `virt`, every boot whose QEMU has the device | the RISC-V IOMMU (`-device riscv-iommu-pci`, first on the bus as `00:01.0`; Sv39x4 to Sv57x4, MSI translation, flat and through MRIFs) | `iommu: riscv unit at 00:01.0, 256 device ids, Sv39x4 second stage, MRIFs` |

- **AMD's unit needs QEMU 10.1 or later.** QEMU translates an emulated
  device's DMA through the unit only with `dma-remap=on`, and an older
  QEMU (Ubuntu 24.04's 8.2) refuses a DMA to an unmapped page without
  logging it. On an older QEMU the launcher keeps Intel's unit and says
  so once.
- **The RISC-V IOMMU needs QEMU 9.2 or later.** CI's image has it, Ubuntu
  24.04's 8.2 does not. Without it the launcher says so once and boots
  without; `/proc/iommu` then says `none`.
- **The `pc` machine has no IOMMU.** `-device intel-iommu` is not for it
  (`MYOS_X86_MACHINE=pc`, `docs/platform.md`).
- **The UEFI boots run OVMF from edk2-stable202505** (the launcher's
  `OVMF` constant). The edk2-stable202605 build raises an invalid-opcode
  exception inside the firmware on a `q35` with the IOMMU and an xHCI
  controller, before any boot loader runs.

A board without a DMAR table prints no boot line, and `/proc/iommu` says
`none`.
