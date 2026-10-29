# Raspberry Pi

myos runs on a **Raspberry Pi 4** and boots a **Raspberry Pi 3** to a
serial shell ("Raspberry Pi 3", further down). The Pi 5 is not supported
yet.

The Pi 4 runs behind its community UEFI firmware
([pftf/RPi4](https://github.com/pftf/RPi4)). The firmware starts Limine
from a USB stick like any UEFI machine, and the kernel reads the board
from the firmware's device tree.

The Pi 4 is **untested on hardware so far**. No QEMU model runs its
firmware, so the kernel's side is checked on QEMU `virt` with a device
tree shaped like the Pi's.

What should work on the Pi 4:

- the four cores, the GIC-400, the generic timer and the serial console;
- the HDMI framebuffer the firmware sets up (`docs/fb.md`);
- everything that runs from the initramfs;
- USB (`docs/usb.md`) through the VL805 xHCI controller behind the
  BCM2711's PCIe bridge: a keyboard on the HDMI console, a mouse, memory
  sticks, hubs;
- the SD card (below);
- Wi-Fi: the CYW43455 as `/dev/sdio`, driven by the `wifi` daemon
  ("Broadcom's radios" in `docs/wifi.md`).

What does not work yet: Ethernet (GENET).

The Pi has no RTC. Its wall clock counts from the epoch at boot until
`date` or `settimeofday` sets it.

## Running it on a Pi 4

```sh
cargo run -- rpi4        # writes target/rpi4.img
sudo dd if=target/rpi4.img of=/dev/sdX bs=4M conv=fsync
```

`target/rpi4.img` is `target/aarch64.img` without QEMU's device tree, so
the kernel gets the firmware's, and with `platform=dt` on the command
line: the firmware can offer ACPI too, but myos reads the device tree
(`docs/platform.md`).

The firmware:

1. Format a microSD card with one FAT32 partition and unzip the latest
   [pftf/RPi4 release](https://github.com/pftf/RPi4/releases) onto it.
2. Boot the Pi with it once and press Esc for the setup. Set **Device
   Manager → Raspberry Pi Configuration → ACPI / Device Tree → System
   Table Selection** to **Devicetree** (or **ACPI+Devicetree**). Save and
   reset.
3. With the USB stick plugged in, the firmware boots it
   (`EFI/BOOT/BOOTAA64.EFI`, Limine). You can also pick it in the Boot
   Manager.

The serial console: a 3.3 V USB serial adapter on GPIO 14 (TX, pin 8) and
15 (RX, pin 10), ground on pin 6, at 115200 8N1. The kernel uses the UART
that `/chosen` `stdout-path` names: the PL011 when `config.txt` has
`dtoverlay=miniuart-bt` (pftf's does), else the mini UART. It keeps the
firmware's baud rate on the PL011, which runs from a 48 MHz clock, not
the 24 MHz the kernel's own divisors assume.

## How the kernel finds the board

`kernel/src/dt.rs` skips nodes whose `status` is not `okay` and
translates each `reg` through the `ranges` of the buses above it (the
Pi's peripherals are at bus address `0x7e00_0000`, CPU address
`0xfe00_0000`). Device MMIO is mapped in the kernel's higher-half window
(`kernel/src/arch/aarch64/paging.rs`), so devices near 4 GiB do not
collide with user space. A board without a PCIe host bridge the kernel
can drive boots without PCI.

## PCIe and USB

The BCM2711's PCIe bridge (`brcm,bcm2711-pcie`) is not ECAM: it shows one
function's configuration space at a time through a window
(`kernel/src/arch/aarch64/pci.rs`). The kernel uses the bridge as the
UEFI firmware left it after its own USB boot, with the VL805's firmware
loaded. On buses 0 and 1 it scans slot 0 only: addressing another
slot on bus 0 hangs the memory controller.

The VL805 reaches only the low 3 GiB (the bridge's `dma-ranges`), and its
DMA does not see the CPU's caches. So the xHCI driver's DMA memory comes
from the low 3 GiB, whatever the firmware's "Limit RAM to 3 GB" setting,
and the driver syncs the caches around each hand-over (`docs/pci.md`
"DMA").

## The SD card

The `sdmmc` module (`modules/sdmmc`) makes the card `/dev/mmcblk0`, a
disk like any other, with its partitions as `/dev/mmcblk0/pN`
(`docs/install.md`). Its card layer is shared by two controller drivers:

- **Pi 4: the EMMC2** (`brcm,bcm2711-emmc2`), a standard SDHCI
  controller. The same driver is checked with QEMU's `sdhci-pci` on every
  arch (the `sdmmc` test); it is untested on the Pi 4 itself. Every
  register access is 32 bits wide, because the Broadcom controllers lose
  narrower writes.
- **Pi 3: the BCM2835's SD host controller** (`brcm,bcm2835-sdhost`).
  Broadcom does not document it; the driver follows Linux's `bcm2835`
  MMC driver and QEMU's model. The Pi 3's SDHCI controller is its Wi-Fi's
  (SDIO, below).

Each controller takes a single card, on the 4-bit bus, at high speed
(50 MHz) when the card and controller allow it, else 25 MHz. The clocks
come from the firmware (`rpi_firmware`, below). SDHCI moves blocks by
ADMA2; the sdhost by the CPU through its FIFO (its DMA and the UHS-I
modes are issue #576).

`/dev/mmcblk0/info` ends with `speed high|default clock HZ dma
adma2|none irq on|off`, which the `sdmmc` test checks.

### SDIO: `/dev/sdio`

An SDIO card is not a disk. The module brings it up with the `sdio`
crate (`modules/sdmmc/sdio`, host-tested with `cargo test -p sdio`), then
hands it to a driver that is a program, as `/dev/sdio/N`, the way
`/dev/pci` hands over a PCI function.

The Pi 3's and Pi 4's Arasan SDHCI (`brcm,bcm2835-sdhci`, the tree's
`mmcnr`) carries the Wi-Fi chip: a CYW43438 on the Pi 3B, a CYW43455 on
the 3B+ and the 4. The module first raises the chip's `WL_REG_ON` through
the firmware's GPIO expander (`/proc/firmware` shows it). A slot whose SD
card does not answer is tried as SDIO too. QEMU has neither the expander
nor an SDIO card, so SDIO is untried, like the rest of the Pi 4.

| File | What |
|------|------|
| `info` | `id MMMM:PPPP` (the CIS's manufacturer and product: `02d0:a9bf` a CYW43455, `02d0:a9a6` a CYW43438), `functions N`, `bus 1\|4`, `clock HZ`, `controller sdhci mmio 0x...`, `block F N` for each function with a block size, `irq on\|off\|none`, `held` while a program drives it |
| `ctl` | held by one program at a time (root only) from its `open` to its last close; reads as `info`; takes `blocksize F N` (function F's block size) and `irq on` / `irq off`. The files below are there while it is held |
| `fN` | function N's registers, the file position the register address (17 bits): one byte is a CMD52, more a CMD53 with the address incrementing (byte mode up to 512 bytes, block mode for a whole number of blocks; 64 KiB at most) |
| `fNfifo` | the same at a fixed address (a FIFO) |
| `irq` | a read waits for the card's interrupt and returns how many came since the last read (8 bytes, little endian); `poll` says one is pending. The interrupt stays masked from the one that fired until a write to `irq`, once the driver quietened the card. The card's own enables (the CCCR's `IENx`) are the driver's, through `f0` |

The CPU moves every transfer through the controller's buffer, so a
program reaches no memory through the card. The `wifi` daemon drives the
Pi's chip this way (`docs/wifi.md`, "Broadcom's radios"). The `sdio` test
checks each card's `info`.

## The VideoCore firmware

The GPU's firmware owns the board's power domains and clocks. The ARM
asks it through the **mailbox property interface** (`brcm,bcm2835-mbox`,
channel 8): a buffer of tags the firmware answers in place.

The `rpi_firmware` module (`modules/rpi_firmware`) publishes this as the
service `rpi_firmware`, a `RpiFirmwareOps` table (`modules/abi`):
`property` runs a tag list, and `power_on` and `clock_rate` wrap the
common tags. `dwc2` powers the USB controller through it, and `sdmmc`
reads its clocks.

`/proc/firmware` shows the firmware's and board's revisions, the EMMC,
UART, ARM and core clocks, and whether the USB controller is powered.
QEMU's `raspi3b` models the property channel, so the Pi 3's CI boot
checks the module (the `rpi_firmware` test). On a board without the
mailbox the module says `rpi_firmware none` and publishes nothing.

## Raspberry Pi 3

A Raspberry Pi 3 (BCM2837, four Cortex-A53) has no UEFI firmware. Its
GPU firmware starts an ARM stub and a kernel from the SD card. myos uses
[U-Boot](https://u-boot.org/) as that kernel, and U-Boot runs Limine as
a UEFI application.

QEMU models the board (`raspi3b`), so this boot is checked there, by CI
too (the smoke tests it can run, `docs/testing.md` "In CI"). It is
**untested on hardware so far**.

```sh
cargo run -- rpi3        # builds the boot files and boots QEMU raspi3b
cargo run -- rpi3 test --tags smoke --skip partitions,ext2_disk,fstab_mount,fileio,boot_rc,heap_mini
                         # CI's boot test of it
```

The kernel's console (the mini UART) is on stdio. U-Boot's console (the
PL011) goes to `target/rpi3/u-boot.log`.

What works:

- the four cores, the generic timer and the serial console;
- the framebuffer the firmware sets up;
- everything that runs from the initramfs;
- power-off and reboot;
- the SD card (above);
- USB through the DWC2 controller (below): a keyboard, a mouse, a stick;
- untried, like the Pi 4's: Wi-Fi, the 3 B's CYW43438 or the 3 B+'s
  CYW43455 as `/dev/sdio`, driven by the `wifi` daemon ("Broadcom's
  radios" in `docs/wifi.md`).

What does not work yet: Ethernet. The LAN9514 behind the USB hub needs a
class driver. The Pi 3 has no RTC either.

### USB: the DWC2 controller

The Pi 3's USB is Synopsys's DesignWare USB 2.0 OTG core
(`brcm,bcm2708-usb`, one root port, 8 host channels). The `dwc2` module
(`modules/dwc2`) drives it as a host: a controller driver of the USB
core (`docs/usb.md`), so everything above it is the same as on xHCI.

Each transfer takes a host channel, with DMA through a bounce buffer of
the module's, since the Pi's DMA does not see the caches. A low- or
full-speed device behind a high-speed hub goes through split
transactions; on a real Pi 3 that is every keyboard and mouse, behind the
LAN9514's hub. QEMU's hub is full-speed, so CI does not exercise the
split path. No isochronous transfers yet (issue #608).

`cargo run -- rpi3` attaches QEMU's hub to the root port with the stick,
the mouse and (in a test boot) the serial adapter behind it. The
`usb_disk`, `mouse`, `usb_keyboard` and `usbdev` tests run in the Pi 3
CI boot.

### Boot files

`boot/rpi3/build.sh`, which `cargo run -- rpi3` runs, builds the boot
files into `target/rpi3`:

- `armstub8-myos.bin` (`boot/rpi3/armstub8.S`, myos's own). Every core
  leaves it at EL1, because Limine refuses EL2 on a core without VHE.
  The stub stays at EL2 as a minimal PSCI for power-off and reboot
  (`docs/power.md`).
- `u-boot.bin`: U-Boot (`UBOOT_VERSION` in `boot/rpi3/versions.env`,
  `rpi_3_defconfig`), built with GCC's AArch64 cross compiler
  (`gcc-aarch64-linux-gnu`), because a clang build crashes at run time.
  It starts `EFI/BOOT/BOOTAA64.EFI` from the first disk with the
  firmware's device tree.
- `bcm2710-rpi-3-b.dtb`: the Pi 3's device tree from the firmware
  release `FIRMWARE_TAG`. QEMU has no GPU firmware, so the launcher does
  its part (`qemu.dtb`, `armstub8-qemu.bin`).
- `myos.img`: the same disk as `target/rpi4.img`, padded to a power of
  two for QEMU's SD card.

### How the kernel finds the Pi 3

The kernel reads the device tree, like the Pi 4's. The interrupt
controllers are Broadcom's two, not a GIC
(`kernel/src/arch/aarch64/bcm2836.rs`, `docs/interrupts.md`): a per-core
local controller, and behind it the BCM2835's for the peripherals. The
console is the mini UART that `stdout-path` names.

### On a real Pi 3

Put these on a FAT32-formatted SD card:

- `bootcode.bin`, `start.elf`, `fixup.dat` and `bcm2710-rpi-3-b.dtb`
  from the
  [firmware release](https://github.com/raspberrypi/firmware/tree/master/boot)
  (`FIRMWARE_TAG`);
- `target/rpi3/armstub8-myos.bin` and `target/rpi3/u-boot.bin`;
- a `config.txt`:

```
arm_64bit=1
enable_uart=1
kernel=u-boot.bin
armstub=armstub8-myos.bin
```

The myos disk goes on a USB stick
(`sudo dd if=target/rpi3/myos.img of=/dev/sdX bs=4M conv=fsync`). U-Boot
boots it when the SD card has no `EFI/BOOT` of its own. myos runs from
the initramfs, sees the SD card (`/dev/mmcblk0`) and the USB stick, and
mounts the stick's ESP and data partition as `/boot` and `/data`. That is
what the QEMU boot shows; it is untried on a real Pi 3.

The console is the mini UART on GPIO 14 and 15 (the same pins as the
Pi 4's), at 115200 8N1.
