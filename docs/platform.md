# The platform description and ACPI

Nothing about the board is hard-coded in myos. At boot, before the heap
and the first console output, `kernel/src/platform.rs` builds one
description of the board from the firmware's ACPI tables and device tree.
The arch code takes its device bases from that description and from
nowhere else (`arch::apply_platform`). Read this page when a board does
not boot, when `/proc/platform` looks wrong, or when you add a component
the kernel must find. The last section covers the firmware's AML, which
the acpi module runs on x86_64 for the PCI interrupt routes and the
power-off.

## Sources

Two sources fill the description, in this order:

1. **The ACPI static tables** (`kernel/src/acpi.rs`). They are used when
   the firmware hands Limine an RSDP and the arch prefers them
   (`arch::PREFER_ACPI`: x86_64 and aarch64). riscv64 does not prefer
   them: its ACPI tables are younger than the firmware that runs here.
   The kernel reads only the tables that name the platform, and no AML:
   - the MADT: the CPUs; the local APIC, or the GICD, GICC, GICR and ITS
     entries;
   - the MCFG: the PCIe ECAM and bus range;
   - the SPCR: the console UART and its interrupt;
   - the GTDT: the arm timer interrupts;
   - the FADT's arm boot flags: PSCI.

   The reading allocates nothing; the tables are read in place through
   the HHDM. `modules/acpi` keeps `/proc/acpi` and the ACPI power
   methods (`_S5` power-off, the reset register: `docs/power.md`).
2. **The device tree** (`kernel/src/dt.rs`, the `fdt` crate, MPL-2.0),
   when Limine hands one over. It fills what nothing described yet. It
   is also compared against what the tables described: when both
   describe a component differently, the kernel flags it rather than
   silently picking either (`[WARN] platform: acpi and dt differ on ...`
   at boot, `differs` in `/proc/platform`). Nodes whose `status` is not
   `okay` are skipped. A `reg` is translated through the `ranges` of the
   buses above it: the Raspberry Pi 4's peripherals, for example, sit at
   bus address `0x7e00_0000` behind a `simple-bus` (`docs/raspberry-pi.md`).

The kernel command line (`cmdline:` in `limine.conf`) forces one source
when a board's firmware gets the other wrong:

- `platform=acpi` ignores the tree. On riscv64 it is the opt-in: it makes
  the kernel read the tables it would otherwise skip.
- `platform=dt` ignores the tables.

### Firmware without a device tree

The EDK2 firmware boots (AAVMF, RISC-V EDK2) hand Limine no tree. The
host tool therefore dumps QEMU's tree (`-machine ...,dumpdtb`) with the
same machine options, firmware and `-smp` as the boot, since QEMU builds
a board with ACPI tables differently. It writes it into the ESP as
`boot/virt-aarch64.dtb` / `boot/virt.dtb`, and `limine.conf` passes it
with `global_dtb`. AAVMF also publishes ACPI tables, so the aarch64 boots
exercise the agreement check (`source acpi+dt`).

## Components

| Component | ACPI | Device tree (`compatible`) |
|------|------|------|
| CPUs | MADT enabled LAPIC / x2APIC / GICC entries | `/cpus` children |
| Interrupt controller | MADT: LAPIC address; GICD (with the GIC version) + GICC → GICv2, GICD + GICR → GICv3 | the nodes of the arch's drivers (`docs/interrupts.md` "Interrupt controllers": the GICs, the Raspberry Pi 3's two, `docs/raspberry-pi.md`, the PLIC) |
| Console UART | SPCR: interface type (16550 or PL011 / SBSA), address, GSIV | the one `/chosen` `stdout-path` names (through `/aliases`), else the first `arm,pl011`, else the first `ns16550a` / `ns16550` / `snps,dw-apb-uart` / `brcm,bcm2835-aux-uart` (the Pi's mini UART); + `interrupts` |
| RTC | — | `arm,pl031`, `google,goldfish-rtc` (optional: without one the wall clock counts from the epoch at boot until `settimeofday` sets it) |
| Timers | GTDT: EL1 physical and virtual timer INTIDs | `/cpus` `timebase-frequency` (riscv64 `time` CSR rate) |
| PCIe host bridge | MCFG: ECAM base, bus range | `pci-host-ecam-generic`: `reg` (the ECAM window), `bus-range`; `brcm,bcm2711-pcie` (aarch64, the Raspberry Pi 4's): `reg` (its registers) |
| PCIe MMIO windows, INTx routing | — (in AML) | `ranges` (the 32-bit entry on aarch64, the 64-bit one on riscv64; CPU and bus address, which may differ), `interrupt-map` / `interrupt-map-mask` |
| DMA (`kernel/src/dma.rs`, `docs/pci.md` "DMA") | — (devices coherent, all of memory) | the host bridge's `dma-ranges`, a node's buses' `dma-ranges`; `dma-coherent` / `dma-noncoherent` on the node or a bus above |
| The IOMMU (`kernel/src/iommu`, `docs/iommu.md`) | aarch64: the IORT's SMMUv3 node and the root complex's ID mapping | `arm,smmu-v3` (`reg`, `interrupts` by `interrupt-names`, `dma-coherent`), the PCIe host's `iommu-map`; x86_64's VT-d units are the DMAR table's, read by `iommu/vtd.rs` itself |
| virtio-mmio transports | — | `virtio,mmio`, for modules through `KernelApi::dt_mmio_find`, in ascending address order |
| PSCI (aarch64 power-off and reset, `docs/power.md`) | FADT arm boot flags: PSCI compliant, HVC or SMC | `arm,psci-1.0` / `arm,psci-0.2` / `arm,psci`: `method` |

The tree's interrupt specifiers are decoded by the driver of their
interrupt parent (`irqchip::xlate`, `docs/interrupts.md`).

## What each arch needs

- **aarch64**:
  - an interrupt controller it has a driver for: a GICv2 (the distributor
    and the CPU interface), a GICv3 (the distributor and the redistributor
    region) or a Raspberry Pi 3's two controllers;
  - a PL011 or a 16550, with its register stride and access width (the
    tree's `reg-shift` / `reg-io-width`, the SPCR's access size);
  - a PCIe host bridge, when it has one, with a 32-bit MMIO window. A
    window the kernel cannot drive leaves the board without PCI.
- **riscv64**: a PLIC, a 16550, the timebase, and a host bridge with a
  64-bit window.
- **x86_64** needs nothing:
  - the LAPIC base comes from its MSR and the console is COM1;
  - PCI configuration space is the MCFG's ECAM window when the
    description has one (QEMU `q35`, every PCIe PC). `arch/x86/pci.rs`
    maps a bus's megabyte of it on first use, once the heap's frames
    exist. Without an MCFG it is port 0xCF8.

  So a PC without ACPI tables boots too. QEMU's `pc` machine publishes
  neither an SPCR nor an MCFG: its description is the CPUs and the LAPIC.

A missing source or component stops the boot with `fatal: platform:
...`, printed on the UART at QEMU `virt`'s address. That address is the
one assumption left, so that the message has somewhere to go.

## The QEMU machines

- **x86_64**: the boots (`cargo run`, the tests, CI) are the PCIe PC,
  `q35`, with Intel's IOMMU (`-device intel-iommu`, `docs/iommu.md`).
  `MYOS_X86_MACHINE=pc cargo run -- test-mini` boots QEMU's default
  machine instead: no ACPI description of PCI and no IOMMU.
- **aarch64 with a GICv3**: QEMU `virt` with `gic-version=3`
  (`MYOS_AARCH64_GIC=3`, `docs/interrupts.md` "GICv3"). These boots have
  no ACPI tables (`acpi=off`): the firmware hands the device tree alone
  (`source dt`). CI therefore boots one aarch64 board described by the
  tree only, and that board has the PL061 (`docs/interrupts.md`
  "Interrupt domains").

## `/proc/platform`

`/proc/platform` shows the description, one line per component. Each
line ends in the source it came from:

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
dma mmio 0xa003e00 coherent
dma pci 00:02.0 coherent
```

A window whose CPU and bus addresses differ shows both: `pci mmio32
0x600000000 0x40000000 bus 0xc0000000` on a Raspberry Pi 4. So does a
device whose DMA reaches only part of memory: `dma pci 01:00.0 0x0
0xc0000000 noncoherent` is the Pi 4's USB controller, which reaches the
low 3 GiB and is not coherent.

## AML

On x86_64 the acpi module runs the firmware's AML with the `acpi` crate's
interpreter (`modules/acpi/src/aml.rs`). The crate is used unmodified
over the module ABI: memory through `phys_map`, the I/O ports, PCI
configuration space. The module:

1. calls `\_PIC(1)` (the OS uses the APIC);
2. runs the devices' `_STA` and `_INI`;
3. reads every PCI bridge's `_PRT`, through its link devices' `_CRS`
   where the firmware has them, and hands each route to the kernel
   (`KernelApi::pci_intx_route`: bus, device, pin, GSI, trigger and
   polarity);
4. evaluates `\_S5` for the power-off.

A root bridge (`_HID` or `_CID` `PNP0A03`, `PNP0A08`) is on its `_BBN`'s
bus. A PCI-to-PCI bridge's routes are for its secondary bus. A function
behind a bridge with no `_PRT` of its own is found through the
PCI-to-PCI swizzle (`(pin + device) mod 4` per bridge,
`kernel/src/arch/x86/intx.rs`). `/proc/acpi/aml` reports what it found:

```
prt \_SB.PCI0 bus 0 routes 128
routes 128
_S5 0 0
```

The interpreter is young and panics at what it does not implement. It
therefore runs in a thread of its own, which a panic parks, and the
module's init waits for it 5 seconds at most. When it fails:

- nothing is routed, and the drivers of functions without MSI poll;
- `_S5` comes from the scan below;
- `/proc/acpi/aml` says why, and the `source:` line of `/proc/acpi/s5`
  says which one gave `_S5` (`AML interpreter`, `AML _S5 scan`).

### The `_S5` scan

The other arches, and x86_64 when the interpreter failed, resolve
`Name (_S5_, Package …)` in the DSDT with a byte scan, for
`/proc/acpi/s5` and the ACPI power-off (`docs/power.md`). The scan finds
the name by its bytes: the segment `_S5_` right after a NameOp (`0x08`;
the root prefix `\` may come between), followed by a PackageOp. It does
not walk the opcodes: staying in step would take every one of them (a
`0x08` in a buffer is no NameOp).

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

The scan decodes the PkgLength encoding. It executes nothing.
