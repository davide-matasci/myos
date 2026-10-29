# The IOMMU

## The IOMMU

A PCI device reads and writes memory by the addresses its driver gives
it, and a program driving one through `/dev/pci` gives them itself: a bug
in it, or its device's own firmware, could reach kernel memory or another
process's pages. The IOMMU (`kernel/src/iommu`, issue #501) is the
platform's unit between the devices and memory, and the kernel programs
it so that:

- a device a kernel module drives stays in passthrough: the *identity
  domain* maps all of memory one-to-one, with the largest pages the unit
  has, so the drivers need nothing from it;
- a function a program holds through `/dev/pci` gets a domain of its own
  (`KernelApi::iommu_attach`, ABI 49, at the `ctl` open), holding only the
  DMA buffers the program asked for (`iommu_map`, at each `dma PAGES`,
  and the ones kept from the last driver), each at its physical address,
  so the numbers `ctl` shows stay the device's; closing `ctl` puts the
  function back in passthrough (`iommu_detach`);
- a DMA anywhere else is refused by the unit and recorded: the unit's
  fault event is an interrupt (an MSI to a device vector), its handler
  reads the fault recording registers. `/proc/iommu` lists the units,
  the domains with what they map, `faults N` and the last one
  (`last BB:SS.F read|write ADDRESS reason R`).

A device's interrupts are messages it writes too (MSIs, to the local
APIC's address). On x86_64 the unit remaps them: each message a function
is given names an entry of an interrupt remapping table that holds the
vector and answers to that function only, so a function raises its own
vector and no other's (`iommu/x86.rs` knows both units' message formats;
`arch/x86/interrupts.rs` programs the devices' messages, `ioapic.rs` the
I/O APIC's pins the same way). VT-d names the entry in the message's
address (the *remappable* format), in one table per unit (`intremap
N/256` on its `/proc/iommu` line, the entries given out; `queued` there:
it invalidates through its queue) whose entry verifies the sender's bus,
device and function, and refuses a plain-format message once the table is
in place (`GCMD.CFI` clear). AMD-Vi names it in the message's data (bits
10:0), in a table of the device's own (`intremap N table(s)` on its line):
a device without one has its interrupts aborted. A remapped
level-triggered pin (a PCI INTx line) is ended at the I/O APIC's EOI
register as well as the LAPIC (`ioapic::eoi`): the LAPIC's EOI clears the
pin's Remote IRR only for an interrupt delivered as level-triggered with
the vector the entry holds, and an AMD-Vi entry holds its index there
(QEMU's AMD unit delivers every remapped interrupt as edge-triggered), so
the line would go silent after its first interrupt. A refused message is
recorded like a DMA on VT-d (reason 0x20 and up, 0x26 another function's
entry, the address its handle); QEMU's AMD unit logs none. QEMU's units
let plain-format messages through (hardware refuses them), so
`pcidev_smoke` checks the entry instead: the device's message rewritten
to the next entry is not delivered (and counted, on VT-d), its own arrives
again afterwards. On aarch64 and riscv64 a message is a write to the MSI
controller's doorbell (the ITS's `GITS_TRANSLATER`, the GICv2m frame's
`MSI_SETSPI_NS`, the boot hart's IMSIC page), a DMA like any other, which
the unit sees when the function is in a domain of its own
(`iommu::msi_message`, when its interrupt is routed):

- On aarch64 the doorbell's page is mapped into the domain, at its own
  address (the SMMUv3's MSI translation stays unused). The ITS tells the
  devices apart by their requester id, so a confined function raises its
  own events and no other's. A GICv2m frame takes any SPI of its range it
  is sent, so a confined function could raise another device's MSI there:
  spurious interrupts, which handlers take as such (they check their
  device's status); the frame has nothing to tell the senders apart, and
  INTx instead would take MSI from the confined functions (issue #590).
- On riscv64 the RISC-V IOMMU translates the function's messages (its
  device context's MSI page table, flat, chapter 2.3 of the
  specification) where it has memory-resident interrupt files
  (`CAP.MSI_MRIF`; `mrif` on its `/proc/iommu` line): the IMSIC page's
  writes go to an MRIF of the function's own, a 512-byte page in memory
  holding a pending and an enable bit per identity, one identity of it
  enabled, which the unit *notices* with a message of its own raising the
  function's IMSIC identity (its domain's line says `mrif ID pending
  0xBITS`, the file's first 64 pending bits). Whatever the function writes
  it raises that identity or nothing; the IMSIC's page itself is not in
  the domain. The kernel clears the file's pending bits when it claims the
  notice, before the handlers run, so the next message is noticed again.
  A unit without MRIFs gets the IMSIC page mapped into the domain as on
  aarch64, the IMSIC then taking any identity. `pcidev_smoke` rewrites the
  device's message to the console UART's identity: it lands as a pending
  bit of the device's file and nothing arrives, the device's own arrives
  once it is put back.

A DMA to the interrupt controller's other registers is refused like any
other, which the smoke checks too.

| Arch | Unit | Where it is described |
|---|---|---|
| x86_64 | Intel VT-d (`iommu/vtd.rs`): a root table per unit (one entry per bus), a context table per bus (one entry per device and function), second-level page tables of the CPU's shape (3 levels for 39-bit addresses, 4 for 48, 2 MiB and 1 GiB superpages where `CAP.SLLPS` has them), invalidation through the queue where the unit has one (`ECAP.QI`; the registers otherwise; whole: the context cache, the IOTLB, the interrupt entry cache), interrupt remapping (`ECAP.IR` and the DMAR's `INTR_REMAP` flag; a table of 256 entries per unit, one per message or I/O APIC pin routed, extended interrupt mode where the unit has it, `ECAP.EIM`, and the BSP runs in x2APIC mode), the tables' cache lines flushed for a unit whose walks do not snoop (`ECAP.C` clear) | the ACPI DMAR table: each DRHD's registers and device scopes (endpoints and bridges), the `INCLUDE_PCI_ALL` one for the rest |
| x86_64 (AMD) | AMD-Vi (`iommu/amdvi.rs`, specification revision 3): a flat device table (a 32-byte entry per device id, for the buses with a function on them), *passthrough* for every function (mode 0: no table walked) and a 4-level page table of AMD's format for one a program holds (a domain id each); the command buffer for invalidations (an entry, a domain's pages, an interrupt table, everything where the unit has `IASup`), each batch ended by a completion wait; the event log for the refused DMAs, its interrupt the MSI of the unit's own PCI function (class 08.06, claimed by the kernel); an interrupt remapping table per device (4-byte entries, no guest mode) | the ACPI IVRS table: each IVHD's registers, own device id and device entries (all, select, range, the I/O APIC as a special device) |
| aarch64 | Arm SMMUv3 (`iommu/smmuv3.rs`): a linear stream table, one entry per requester id, *bypass* for every function (no table walked) and a *stage-2* entry (VMSAv8, 4 KiB granule, a VMID per domain) for one a program holds; the command queue for invalidations and syncs, the event queue (its interrupt, which the SMMU pulses, taken edge-triggered) for the refused DMAs; the tables and queues written back for a unit that does not snoop (`IDR0.COHACC` clear) | the IORT's SMMUv3 node (registers, the event and global error GSIVs) and the root complex node's ID mapping to it, or the tree's `arm,smmu-v3` node and the PCIe host's `iommu-map`: a `platform::Iommu` component, `/proc/platform`'s `iommu smmuv3 ...` line |
| riscv64 | the RISC-V IOMMU (`iommu/riscv.rs`, specification 1.0): a three-level device directory, one 64-byte context per device id (32 bytes for a unit without MSI translation), *bare* for every function of a populated bus (no table walked) and a *second-stage* context (a G-stage table in the CPU's Sv39x4 or Sv48x4 format, a GSCID per domain) for one a program holds; the command queue for invalidations and fences, the fault queue for the refused DMAs, read when `/proc/iommu` is (the PCI unit signals it by MSI only, which the riscv64 kernel does not take); MSI translation through memory-resident interrupt files for the confined functions (`CAP.MSI_MRIF`, above) | a PCI function of class 08.06 (system peripheral, IOMMU), its registers in BAR 0, claimed by the kernel so `/dev/pci` cannot hand it out; nothing from the tree |

QEMU's `q35` carries the unit (`-device intel-iommu,intremap=on`, 39-bit,
3-level, both superpage sizes, snooping walks, queued invalidation,
interrupt remapping), every x86 boot of the launcher and of CI, or AMD's
under `MYOS_X86_IOMMU=amd` (`-device amd-iommu,intremap=on,dma-remap=on`;
CI's `bios-amd` machine; the boot line `[ OK ] iommu: amd-vi, 1 unit(s),
256 device ids, 4-level tables, interrupts remapped`) on a QEMU of 10.1
or later: it translates an emulated device's DMA through the unit only
with `dma-remap=on`, and an older one (Ubuntu 24.04's 8.2) refuses a DMA
to an unmapped page without logging it, so the launcher keeps Intel's
unit there and says so once, QEMU's `virt` an SMMUv3 (`iommu=smmuv3`: 44-bit output,
snooping, SPIs 74 to 77 for its queues) on every aarch64 boot, where
`[ OK ] iommu: smmuv3 at 0x9050000, 4096 streams, 44-bit stage 2` is the
boot line, and the riscv64 `virt` the RISC-V IOMMU (`-device
riscv-iommu-pci`, first on the bus as `00:01.0`, Sv39x4 to Sv57x4, MSI
translation, flat and through MRIFs) on every boot whose QEMU has it: the device is QEMU 9.2's and later
(CI's image has it, Ubuntu 24.04's 8.2 has not); without it the launcher
says so once and boots without, `/proc/iommu` saying `none`.
Its boot line: `[ OK ] iommu: riscv unit at 00:01.0, 256 device ids,
Sv39x4 second stage, MRIFs`; `-device intel-iommu` is not for the `pc` machine
(`MYOS_X86_MACHINE=pc`), whose boot has no IOMMU. The UEFI boots run
OVMF from edk2-stable202505 (the launcher's `OVMF` constant): the
edk2-stable202605 build raises an invalid-opcode exception inside the
firmware on a `q35` with the IOMMU and an xHCI controller, before any boot
loader runs. On the `[ OK ]` line at
boot: `iommu: vt-d, 1 unit(s), 3-level tables, 1 GiB superpages, memory
to 0x..., interrupts remapped`; a board without a DMAR table prints
nothing and `/proc/iommu` says `none`.
