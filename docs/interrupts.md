# Interrupts

This page covers how a device's interrupt reaches its handler on each
arch: the root interrupt controllers and their drivers, the interrupt
domains that modules add for secondary controllers, the MSI controllers,
and how a driver asks for an interrupt and gives it back. The
controllers' addresses come from the platform description
(`docs/platform.md`). The IOMMU confines the devices' messages
(`docs/iommu.md`).

## Interrupt controllers

Every arch drives its root interrupt controller through one interface,
`irqchip::IrqChip` (`kernel/src/irqchip.rs`): per-CPU init, routing and
masking a device's interrupt, claim and complete, and IPIs. Each arch
lists its drivers in `arch::IRQCHIPS`:

| Arch | Driver | Device tree `compatible` | ACPI | File |
|------|--------|--------------------------|------|------|
| x86_64 | `apic`: the local APICs and the I/O APIC | — | MADT (also without it) | `arch/x86/apic.rs` |
| aarch64 | `gicv2` | `arm,cortex-a15-gic`, `arm,gic-400`, `arm,cortex-a9-gic` | MADT GICD + GICC | `arch/aarch64/gic.rs` |
| aarch64 | `gicv3` | `arm,gic-v3` | MADT GICD + GICR | `arch/aarch64/gic.rs` |
| aarch64 | `bcm2836`: the Raspberry Pi 3's two controllers | `brcm,bcm2836-l1-intc` + `brcm,bcm2836-armctrl-ic` | — | `arch/aarch64/bcm2836.rs` |
| riscv64 | `plic` (with the SBI's IPIs) | `sifive,plic-1.0.0`, `riscv,plic0` | — | `arch/riscv64/plic.rs` |
| riscv64 | `aia`: the APLIC and the IMSICs (with the SBI's IPIs) | `riscv,aplic` + `riscv,imsics` | — | `arch/riscv64/aia.rs` |

The platform description names the driver (`intc gicv2 ...` in
`/proc/platform`): the first of the arch's drivers whose nodes the tree
has, or the one the MADT names. A device tree interrupt specifier is
decoded by the driver its interrupt parent names (`irqchip::xlate`); a
specifier whose parent no driver knows is not delivered.

To add a controller, write a file implementing `IrqChip` with its
`irqchip::Driver` and add the driver to its arch's `IRQCHIPS`. aarch64's
controllers number their interrupts as the GIC does, which the interrupt
entry relies on: SGI 0 and 1 are the IPIs, PPI 27 and 30 the timers, 32
and up the devices.

### GICv3

The GICv3 driver runs the distributor with affinity routing and the CPU
interface through the `ICC_*` system registers. The boot log says which
GIC is in use (`[INFO] gicv3`). QEMU `virt` gives a GICv3 with
`gic-version=3`:

```sh
MYOS_AARCH64_GIC=3 cargo run -- aarch64 test-mini
```

The launcher dumps the matching tree (`docs/platform.md`). CI boots both
versions (`aarch64`, `aarch64-gicv3`); the GICv3 boots have no ACPI
tables.

### Interrupt domains

A secondary controller, whose lines reach the CPU through one interrupt
of the root's (a GPIO block's pins, a PCIe bridge's MSI), is a module. It
registers an interrupt domain (`KernelApi::irq_domain_register`) and
drives the controller:

- The kernel gives the domain interrupt numbers from 4096 up
  (`irqchip::DOMAIN_FIRST`), line `n` being `first + n`. There are at
  most 8 domains, with 4096 lines between them.
- A driver of a device on a line enables it as any other (`irq_enable`).
  The kernel calls the domain's `unmask` and, when the line's last
  handler goes, its `mask`.
- The module's handler for the controller's own interrupt passes each
  pending line to `KernelApi::irq_dispatch`.
- A device tree specifier whose `interrupt-parent` is the domain's node
  decodes to its line.

A domain stays while the kernel runs, so its module stays loaded.

The first domain is `modules/pl061`, the 8 lines of the ARM PL061 GPIO
controller on QEMU `virt` without ACPI. No driver uses a line yet, so the
module ships for `insmod` and is not loaded at boot. Its probe checks
the path once: a line enabled, its interrupt handled, then given back
and masked. The result shows in `/proc/pl061` (`check given back`) and
`/proc/interrupts`; the `irq_domain` test checks it.

## MSI controllers

On aarch64 and riscv64 a PCI function's MSI-X or MSI is programmed as on
x86_64 (`kernel/src/msi.rs`), with the message the board's MSI
controller gives (`irqchip::MsiChip`). A function without MSI-X or MSI,
or a board without an MSI controller, keeps its INTx.

| Controller | Arch | Code | A message raises | `/proc/platform` |
|---|---|---|---|---|
| GICv3 ITS | aarch64 | `arch/aarch64/gic_msi.rs` | an LPI (256 from 8192), on the BSP | `msi its ...` |
| GICv2m frame | aarch64 | `gic_msi.rs` | an SPI of the frame's range, edge-triggered | `msi v2m ...` |
| RISC-V AIA's IMSIC | riscv64 | `arch/riscv64/aia.rs` | an identity of the boot hart's interrupt file | `intc aia ...`, `msi imsic ...` |

With the AIA, the APLIC delivers wired source `n` as IMSIC identity `n`,
so the tree's numbers stay the interrupt numbers; the identities above
the sources are the MSIs.

In QEMU:

- aarch64 `virt` has the ITS with a GICv3 and a GICv2m frame with a
  GICv2; CI's two aarch64 machines cover both (`[ OK ] msi: its, 256
  LPIs from 8192`, `[ OK ] msi: gicv2m, SPIs 80-143`).
- riscv64 `virt` has the AIA with `aia=aplic-imsic`, the launcher's
  default. `MYOS_RISCV_INTC=plic` boots the PLIC alone, INTx only. CI
  boots both (`riscv64`, `riscv64-plic`). The boot line is `[ OK ] msi:
  imsic, identities 97-255`.
- The Raspberry Pi 3 has no PCI.

## Device interrupts

`kernel/src/irq.rs` keeps the handlers: 64 on every arch, as many as
x86_64 has device vectors.

### PCI functions

A module asks for a PCI function's interrupt with
`KernelApi::pci_irq_enable`. The kernel picks MSI-X entry 0, else the
function's MSI, else its INTx line, and tells the module which
(the entry, `MYOS_IRQ_MSI` or `MYOS_IRQ_INTX`).

| Arch | MSI delivery | INTx | Numbering |
|------|-----|------|------|
| x86_64 | a LAPIC vector on the BSP (`DEVICE_VECTOR_BASE` 48 + n), through the IOMMU's interrupt remapping where there is one | an I/O APIC pin, from the firmware's `_PRT` (`docs/platform.md` "AML"); none, and the driver polls, without a route | vector |
| aarch64 | the board's MSI controller | a GIC SPI from the tree's `interrupt-map` (QEMU `virt`: 35..38) | INTID |
| riscv64 | the IMSIC | a PLIC or APLIC source from the `interrupt-map` | source or identity |

On x86_64 a BSP with an x2APIC id above 255 is reached only through
interrupt remapping. An INTx line's functions whose drivers poll or use
MSI have their INTx turned off, so none holds the level line asserted.

### Board devices

A board device's own interrupt goes through `irq::enable` (for modules,
`KernelApi::irq_enable`). The handler is registered before the line is
unmasked.

- On aarch64 and riscv64 the number is the device tree's (a module gets
  it from `MmioDevice::irq`) or the SPCR's.
- On x86_64 it is a legacy ISA IRQ, routed through the MADT's first I/O
  APIC with the interrupt source overrides (`arch/x86/ioapic.rs`). The
  8259s stay masked.

The console's input uses it: the UART (COM1's IRQ 4), the PS/2 keyboard
(IRQ 1) and mouse (IRQ 12), and the virtio-input devices. A PC without
an MADT I/O APIC routes nothing: CPU 0's tick drains the UART and the
keyboard is polled.

### Handlers and shared lines

Handlers run in interrupt context, with interrupts masked, on CPU 0.

- On INTx the handler must read the device's ISR register, so that the
  level line deasserts before the controller's EOI.
- An INTx line is shared by every slot the swizzle puts on it (QEMU
  `virt` has four for all of them). Every handler on the line runs at
  each interrupt, its device saying whether it was the one.

`/proc/interrupts` lists the count per handler, the spurious interrupts,
and on x86_64 the device vectors taken (`vectors: N/64`).

### Giving an interrupt back

`KernelApi::pci_irq_disable` gives a PCI function's interrupt back:
the handler leaves the table, the function's interrupt is turned off,
and once no handler is left the arch frees what it took (x86_64: the
vector, the IOMMU's remapping entry, the I/O APIC pin; aarch64 and
riscv64: an MSI's LPI, SPI or identity). A shared INTx line is only
masked once no handler is left on it.

`KernelApi::irq_disable` gives a board device's interrupt back the same
way, masking the line through the controller (`IrqChip::mask`) or the
domain.

Either call lowers the registration count of the module that enabled the
interrupt. `/dev/pci` gives a function's interrupt back when its driver
closes `ctl` (`docs/pci.md`); the kernel's own drivers keep theirs.

### virtio-net's receive interrupt

virtio-net takes an interrupt on its RX queue only. The handler wakes
the `poll` that `netd` sleeps in on `/dev/net0/data`. The NIC's `ctl`
says `irq on` when this works; on `irq off` (a poll-mode device) `netd`
keeps its 2–10 ms timed polling. Reads of `/dev/netN/data` stay
non-blocking.
