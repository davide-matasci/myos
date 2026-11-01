# The platform description and ACPI

Nothing about the board is hard-coded in myos. At boot, before the heap,
`kernel/src/platform.rs` builds one
description of the board from the firmware's ACPI tables and device tree.
The arch code takes its device bases from that description and from
nowhere else (`arch::apply_platform`). Read this page when a board does
not boot, when `/proc/platform` looks wrong, or when you add a component
the kernel must find. The last section covers the firmware's AML, which
the acpi module runs on x86_64 for the PCI interrupt routes and the
power-off.

## Sources

Two sources fill the description, in this order:

1. **The ACPI static tables** (`kernel/src/acpi.rs`): the MADT, MCFG,
   SPCR, GTDT and the FADT's arm boot flags, read in place, with no AML.
   They are used when the firmware hands Limine an RSDP and the arch
   prefers them (`arch::PREFER_ACPI`: x86_64 and aarch64). riscv64 does
   not: its ACPI tables are younger than the firmware that runs here.
   `modules/acpi` keeps `/proc/acpi` and the ACPI power methods
   (`docs/power.md`).
2. **The device tree** (`kernel/src/dt.rs`, the `fdt` crate, MPL-2.0),
   when Limine hands one over. It fills what nothing described yet. When
   both sources describe a component differently, the kernel flags it
   rather than silently picking either (`[WARN] platform: acpi and dt
   differ on ...` at boot, `differs` in `/proc/platform`). A `reg` is
   translated through the `ranges` of the buses above it, as on the
   Raspberry Pi 4 (`docs/raspberry-pi.md`).

On the kernel command line (`cmdline:` in `limine.conf`),
`platform=acpi` or `platform=dt` forces one source when a board's
firmware gets the other wrong; on riscv64 `platform=acpi` is the opt-in
to the tables.

The EDK2 firmware boots (AAVMF, RISC-V EDK2) hand Limine no tree, so the
host tool dumps QEMU's (`-machine ...,dumpdtb`, same options as the
boot) into the ESP as `boot/virt-aarch64.dtb` / `boot/virt.dtb`, and
`limine.conf` passes it with `global_dtb`. AAVMF also publishes ACPI
tables, so the aarch64 boots exercise the agreement check (`source
acpi+dt`).

## Components

| Component | ACPI | Device tree |
|------|------|------|
| CPUs | MADT | `/cpus` |
| Interrupt controller | MADT (LAPIC; GICv2 or GICv3) | the arch's drivers' nodes (`docs/interrupts.md` "Interrupt controllers") |
| Console UART | SPCR | `/chosen` `stdout-path`, else the first PL011 or 16550-like UART |
| RTC | — | `arm,pl031`, `google,goldfish-rtc` (optional: without one the clock counts from the epoch until `settimeofday`) |
| Timers | GTDT | `timebase-frequency` (riscv64) |
| PCIe host bridge | MCFG | `pci-host-ecam-generic`, `brcm,bcm2711-pcie` (Raspberry Pi 4) |
| PCIe windows, INTx routing | — (in AML) | `ranges`, `interrupt-map` |
| DMA (`kernel/src/dma.rs`, `docs/pci.md` "DMA") | — (coherent, all of memory) | `dma-ranges`, `dma-coherent` / `dma-noncoherent` |
| The IOMMU (`docs/iommu.md`) | aarch64: IORT; x86_64: DMAR / IVRS, read by `kernel/src/iommu` itself | `arm,smmu-v3`, the host's `iommu-map` |
| MSI controller (`docs/interrupts.md` "MSI controllers") | MADT | the host's `msi-map` |
| virtio-mmio transports | — | `virtio,mmio`, for modules through `KernelApi::dt_mmio_find` |
| PSCI (`docs/power.md`) | FADT arm boot flags | `arm,psci*` `method` |

The tree's interrupt specifiers are decoded by the driver of their
interrupt parent (`irqchip::xlate`, `docs/interrupts.md`).

## What each arch needs

- **aarch64**: an interrupt controller it has a driver for (GICv2, GICv3,
  or the Raspberry Pi 3's two), a PL011 or a 16550, and, if the board has
  PCIe, a host bridge with a 32-bit MMIO window.
- **riscv64**: a PLIC or the AIA, a 16550, the timebase, and a host
  bridge with a 64-bit window.
- **x86_64** needs nothing. The LAPIC base comes from its MSR and the
  console is COM1. PCI configuration space is the MCFG's ECAM window when
  there is one (`arch/x86/pci.rs`), port 0xCF8 otherwise. So a PC without
  ACPI tables boots too.

A missing source or component stops the boot with `fatal: platform:
...`, printed on the UART at QEMU `virt`'s address: the one assumption
left, so that the message has somewhere to go.

## The QEMU machines

- **x86_64**: the boots are the PCIe PC, `q35`, with Intel's IOMMU
  (`docs/iommu.md`). `MYOS_X86_MACHINE=pc cargo run -- test-mini` boots
  QEMU's default machine instead: no ACPI description of PCI, no IOMMU.
- **aarch64 with a GICv3** (`MYOS_AARCH64_GIC=3`, `docs/interrupts.md`
  "GICv3") has no ACPI tables (`source dt`). CI therefore boots one
  aarch64 board described by the tree only, and that board has the PL061
  (`docs/interrupts.md` "Interrupt domains").

## `/proc/platform`

`/proc/platform` shows the description, one line per component, each
ending in its source:

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

A window whose CPU and bus addresses differ shows both (`pci mmio32
0x600000000 0x40000000 bus 0xc0000000` on a Raspberry Pi 4), and so does
a device whose DMA reaches only part of memory (`dma pci 01:00.0 0x0
0xc0000000 noncoherent`). The `platform` test in `user/tests/kernel.sh`
checks it (tag `smoke`).

## AML

On x86_64 the acpi module runs the firmware's AML with the `acpi` crate's
interpreter over the module ABI (`modules/acpi/src/aml.rs`). The crate
is patched (`modules/acpi/crate/*.myos.patch`, laid out in
`target/acpi-myos` by `crate/prepare.sh` before kernel/build.rs builds
the module) for what 6.1.1 leaves unimplemented and a ThinkPad T15's
firmware needs: its embedded controller accesses go to the region
handler the module installs, it runs `Match`, it lets `Notify` pass
(nothing listens for notifications), and it reads a `_STA`'s value out
of whatever the method returned (nothing at all counts as present).
It calls `\_PIC(1)`, runs the devices' `_STA` and `_INI`, hands every PCI
bridge's `_PRT` routes to the kernel (`KernelApi::pci_intx_route`), and
evaluates `\_S5` for the power-off. A function behind a bridge with no
`_PRT` is routed through the PCI-to-PCI swizzle
(`kernel/src/arch/x86/intx.rs`). `/proc/acpi/aml` reports what it found
(test `acpi_aml`):

```
prt \_SB.PCI0 bus 0 routes 128
routes 128
_S5 0 0
```

The interpreter is young and panics at what it does not implement, so it
runs in a thread of its own, which a panic parks; the module's init
waits for it 5 seconds at most. When it fails, nothing is routed (the
drivers of functions without MSI poll), `_S5` comes from the scan below,
and `/proc/acpi/aml` says why: for a panic, its message, its place in
the crate's source and what the AML was doing (`interpreter panicked:
not yet implemented at src/aml/mod.rs:1270 (the devices' _STA and
_INI)`), which says what the crate lacks for that firmware. The `source:` line of `/proc/acpi/s5`
says which one gave `_S5` (`AML interpreter`, `AML _S5 scan`).

### Batteries

A laptop's batteries (`PNP0C0A`) and AC adapter (`ACPI0003`) are AML
devices whose methods read the embedded controller (`PNP0C09`), so the
acpi module handles its regions (`modules/acpi/src/battery.rs`): its
data and command/status ports from its `_CRS`, a byte read or written
through the ACPI EC protocol, each step bounded to 100 ms. It installs
the handler before the namespace's `_INI`s and calls the controller's
`_REG (3, 1)` after them. `/proc/acpi/battery`, there when the
firmware has a battery or an adapter, is made at each read: each
battery's `_STA`, `_BIX` (else `_BIF`) and `_BST`, each adapter's
`_PSR`. Capacities and rates are in mWh and mW, or mAh and mA when the
battery says so; a value the battery reports unknown is left out:

```
BAT0 present 1
BAT0 state discharging
BAT0 percent 75
BAT0 remaining 30000 mWh
BAT0 full 40000 mWh
BAT0 design 50000 mWh
BAT0 rate 10000 mW
BAT0 voltage 11100 mV
AC online 0
```

`battery` (`/bin/etc/battery`, `user/c/battery.c`) prints it a line per
device (`BAT0: 75% discharging, 3:00 left`, `AC: offline`; `-r` the file
as it is). As the interpreter may still panic, only its thread runs AML:
it stays after the boot's work to answer the reads (`KernelApi::proc_set_reader`
calls the module in the reading task, which asks the thread and waits 5
seconds at most), and a read after a panic says `error interpreter
stopped:` with the panic's line; a slow answer or a panic names the
embedded controller's timed-out steps too (`ec timeouts N`).
`/proc/acpi/aml` is read live, so a panic after the boot's work shows
there as well. QEMU's PCs have no battery: the x86 test boots add an SSDT with
one (`src/acpi_battery.rs`, test `battery`). The embedded controller
path runs only on real laptops.

### The tables

`/proc/acpi/dump` holds the firmware's tables, each whole (header and
all): the DSDT, then every table the XSDT (or RSDT) lists, in its
order. Each one's length is in its header (bytes 4 to 7), which is how
they are told apart. It is for reading a machine's AML on another one,
when the interpreter stops on something it lacks (`/proc/acpi/aml`):
copy it off (a FAT USB stick, `scp`), split it and disassemble it with
ACPICA's `iasl`:

```sh
python3 - dump <<'PY'
import struct, sys
d = open(sys.argv[1], "rb").read()
i = n = 0
while i + 8 <= len(d):
    sig, size = d[i:i + 4].decode(), struct.unpack_from("<I", d, i + 4)[0]
    open(f"{n:02d}-{sig}.dat", "wb").write(d[i:i + size])
    i, n = i + size, n + 1
PY
iasl -e *-SSDT.dat -d 00-DSDT.dat   # 00-DSDT.dsl
```

Test `acpi_dump`.

### The `_S5` scan

The other arches, and x86_64 when the interpreter failed, find
`Name (_S5_, Package …)` in the DSDT by its bytes (a NameOp, `_S5_`, a
PackageOp) and read its integers (Zero, One, Ones, the Byte/Word/DWord/
QWord prefixes, ReturnOp). It does not walk the opcodes, since staying
in step would take every one of them, and it executes nothing.
