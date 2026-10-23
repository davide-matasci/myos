# Raspberry Pi

myos runs on a **Raspberry Pi 4** (below) and boots a **Raspberry Pi 3**
to a serial shell ("Raspberry Pi 3", further down). The Pi 4 runs behind its community UEFI firmware
([pftf/RPi4](https://github.com/pftf/RPi4)): the firmware starts Limine
from a USB stick like any UEFI machine, and the kernel reads the board
from the firmware's device tree. **Untested on hardware so far**: the
kernel's side is checked on QEMU `virt` with a device tree shaped like
the Pi's (the UART and GIC behind a `simple-bus` whose `ranges` move
them, disabled decoy nodes, `stdout-path` through an alias), but no QEMU
model runs the Pi's firmware, so no CI boot covers it. The Pi 5 is
not supported yet.

What should work: the four cores, the GIC-400, the generic timer, the
serial console, the HDMI framebuffer the firmware sets up (`docs/fb.md`),
everything that runs from the initramfs, and USB (`docs/usb.md`): a
keyboard on the HDMI console, a mouse, memory sticks, hubs, through the
VL805 xHCI controller behind the BCM2711's PCIe bridge, the SD card
(below) and Wi-Fi (the CYW43455 as `/dev/sdio`, driven by the `wifi`
daemon: "Broadcom's radios" in `docs/wifi.md`). What does not (yet):
Ethernet (GENET). The Pi has no RTC: its wall
clock counts from the epoch at boot until `date` or `settimeofday` sets it.

## Building the image

```sh
cargo run -- rpi4        # writes target/rpi4.img
```

`target/rpi4.img` is the AArch64 boot disk with two differences from
`target/aarch64.img`: Limine is not given QEMU's device tree (so the
kernel gets the firmware's), and the kernel command line has
`platform=dt` (the firmware can offer ACPI tables too; myos reads the
device tree, `docs/pci-acpi-smp.md`). Write it to a USB stick:

```sh
sudo dd if=target/rpi4.img of=/dev/sdX bs=4M conv=fsync
```

## The firmware

1. Format a microSD card with one FAT32 partition and unzip the latest
   [pftf/RPi4 release](https://github.com/pftf/RPi4/releases) onto it.
2. Boot the Pi with it once, press Esc for the setup and set **Device
   Manager → Raspberry Pi Configuration → ACPI / Device Tree → System
   Table Selection** to **Devicetree** (or **ACPI+Devicetree**). Save and
   reset.
3. With the USB stick plugged in, the firmware boots it
   (`EFI/BOOT/BOOTAA64.EFI`, Limine), or pick it in the Boot Manager.

## Serial console

A 3.3 V USB serial adapter on GPIO 14 (TX, pin 8) and 15 (RX, pin 10),
ground on pin 6, at 115200 8N1. The kernel uses the UART the device
tree's `/chosen` `stdout-path` names: the PL011 when the firmware's
`config.txt` has `dtoverlay=miniuart-bt` (pftf's does), else the mini
UART. It keeps the firmware's baud rate on a PL011 it finds enabled (the
Pi's runs from a 48 MHz clock, not the 24 MHz the kernel's own divisors
assume).

## How the kernel finds the board

`kernel/src/dt.rs` skips nodes whose `status` is not `okay` and
translates each `reg` through the `ranges` of the buses above it (the
Pi's peripherals are at bus address `0x7e00_0000`, CPU address
`0xfe00_0000`). Device MMIO is mapped in the kernel's higher-half window
(`kernel/src/arch/aarch64/paging.rs`), so the Pi's devices near 4 GiB
do not collide with user space. A board without a PCIe host bridge the
kernel can drive boots without PCI.

## PCIe and USB

The BCM2711's PCIe bridge (`brcm,bcm2711-pcie`) is not ECAM: its
registers show the root port's configuration space at offset 0, and one
other function's at a time through a window at 0x8000, chosen by writing
`bus << 20 | slot << 15 | func << 12` at 0x9000
(`kernel/src/arch/aarch64/pci.rs`; the register layout as FreeBSD's
`bcm2838_pci.c` documents it). The kernel uses the bridge as the UEFI
firmware left it, which brought it up for its own USB boot: the link
trained (bits 4 and 5 of the state register at 0x4068), the windows set,
the root port's bus numbers assigned, and the VL805's firmware loaded.
It scans the buses the root port's subordinate number covers, slot 0
only on buses 0 and 1 (addressing another slot on bus 0 hangs the
memory controller). The 32-bit window is at CPU `0x6_0000_0000`, bus
`0xc0000000`: BARs hold bus addresses, mapped at their CPU address.

The VL805 reaches the low 3 GiB (the bridge's `dma-ranges`), and its DMA
does not see the CPU's caches: the xHCI driver's DMA memory comes from
there, whatever the firmware's "Limit RAM to 3 GB" setting, and it
cleans and invalidates the caches around each hand-over (`dma_sync`,
`docs/pci-acpi-smp.md` "DMA").

## The SD card

The `sdmmc` module (`modules/sdmmc`) makes the card `/dev/mmcblk0`, a
disk like any other (its partitions `/dev/mmcblk0/pN`, `docs/install.md`).
Its card layer brings the card up and moves the blocks (the SD Physical
Layer Specification's commands and registers, from the `sdio-host` crate)
behind the controller's driver:

- the Pi 4's EMMC2 (`brcm,bcm2711-emmc2`) is a standard SDHCI controller,
  the driver QEMU's `sdhci-pci` checks on every arch (the `sdmmc` test);
  untested on the Pi 4 itself. Every register access is 32 bits wide (the
  Broadcom controllers lose narrower writes), and with no base clock in
  its capabilities the divisors assume 250 MHz, so the card never runs
  faster than asked.
- the Pi 3's slot is on the BCM2835's own SD host controller
  (`brcm,bcm2835-sdhost`), which Broadcom does not document: its
  registers as Linux's `bcm2835` MMC driver and QEMU's model use them.
  Its divisors assume the highest core clock, 400 MHz (250 MHz with
  `enable_uart=1`); its pins stay as the firmware set them. The Pi 3's
  SDHCI controller (`brcm,bcm2835-sdhci`) is its Wi-Fi's: an SDIO card,
  below.

The CPU moves the data (no DMA) and the module polls for completion, a
single card per controller at the default speed (25 MHz, 4-bit bus);
DMA, interrupts and the faster modes are issue #549.

### SDIO: `/dev/sdio`

An SDIO card is not a disk: the module brings it up (the `sdio` crate,
`modules/sdmmc/sdio`: CMD5 for its functions and voltage, the relative
address, the CCCR for the 4-bit bus and high speed, the CIS for what it
is; host-tested against a model, `cargo test -p sdio`) and hands it to a
driver that is a program as `/dev/sdio/N`, the way `/dev/pci` hands over
a PCI function. The Pi 3's and Pi 4's Arasan SDHCI (`brcm,bcm2835-sdhci`,
the tree's `mmcnr`) carries the Wi-Fi chip (CYW43438 on the Pi 3B,
CYW43455 on the 3B+ and 4), whose `WL_REG_ON` the module raises through
the firmware's GPIO expander first (`rpi_firmware`: `WL_ON`, line 1, held
low then high; `/proc/firmware` shows it; QEMU's firmware has no
expander and the controller no card). A slot whose SD card does not
answer is tried as SDIO too (QEMU's `sdhci-pci` with an SDIO model, one
day). Untried on hardware so far, like the rest of the Pi 4.

| File | What |
|------|------|
| `info` | `id MMMM:PPPP` (the CIS's manufacturer and product: `02d0:a9bf` a CYW43455, `02d0:a9a6` a CYW43438), `functions N`, `bus 1\|4`, `clock HZ`, `controller sdhci mmio 0x...`, `block F N` for each function with a block size, `irq on\|off\|none` (the card's interrupt at the controller; `none`: no interrupt line), `held` while a program drives it |
| `ctl` | held by one program at a time (root only) from its `open` to its last close; reads as `info`; takes `blocksize F N` (function F's block size, into its FBR, for block-mode transfers) and `irq on` / `irq off`. The files below are there while it is held; letting it go masks the interrupt |
| `fN` | function N's registers, the file position the register address (17 bits): a read or write of one byte is a CMD52, of more a CMD53 with the address incrementing (byte mode up to 512 bytes, block mode for a whole number of the function's blocks, more than one; 64 KiB at most) |
| `fNfifo` | the same at a fixed address (a FIFO) |
| `irq` | a read waits for the card's interrupt and returns how many came since the last read (8 bytes, little endian), `poll` says one is pending; the interrupt stays masked at the controller from the one that fired until a write to `irq` (a byte at least), once the driver quietened the card. The card's own enables (the CCCR's `IENx`) are the driver's, through `f0` |

The CPU moves every transfer through the controller's buffer, so a program
reaches no memory through the card. The `wifi` daemon drives the Pi's chip
this way (`docs/wifi.md`, "Broadcom's radios"). The `sdio` test checks
each card's `info`.

## The VideoCore firmware

The GPU's firmware owns the board's power domains and clocks; the ARM
asks it through the **mailbox property interface** (`brcm,bcm2835-mbox`,
channel 8): a buffer of tags in memory the firmware reads and answers in
place, handed over by its bus address (the GPU's uncached alias of
memory, what the tree's `dma-ranges` give the `dma_alloc` of the
mailbox's node). The `rpi_firmware` module (`modules/rpi_firmware`) does
that and publishes it as the service `rpi_firmware` (a `RpiFirmwareOps`
table, `modules/abi`): `property` runs a tag list, and the helpers
`power_on` (a domain: the USB controller's, `RPI_POWER_USB_HCD`) and
`clock_rate` (`RPI_CLOCK_CORE`, `RPI_CLOCK_EMMC`, ...) wrap the common
tags. The `dwc2` module powers the USB controller through it. What the
firmware says of the board is in `/proc/firmware`: the firmware's and
board's revisions, the EMMC, UART, ARM and core clocks, and whether the
USB controller is powered. QEMU's `raspi3b` models the property channel
(the clocks it reports are the firmware's defaults), so the module's path
is checked by the Pi 3's CI boot (`rpi_firmware` test). A board without
the mailbox has no firmware to talk to: the module says `rpi_firmware
none` and publishes nothing.

## Raspberry Pi 3

A Raspberry Pi 3 (BCM2837, four Cortex-A53) has no UEFI firmware: its
GPU firmware starts an ARM stub and a kernel from the SD card, and myos
uses [U-Boot](https://u-boot.org/) as that kernel, which runs Limine as
a UEFI application. QEMU models the board (`raspi3b`), so this boot is
checked there, by CI too (the smoke tests it can run, `docs/testing.md`
"In CI"); **untested on hardware so far**.

```sh
cargo run -- rpi3        # builds the boot files and boots QEMU raspi3b
cargo run -- rpi3 test --tags smoke --skip partitions,ext2_disk,fstab_mount,fileio,boot_rc,libc,heap_mini
                         # CI's boot test of it
```

The console is on stdio (the kernel's, the mini UART); U-Boot's goes to
`target/rpi3/u-boot.log` (the PL011). What works: the four cores, the
generic timer, the serial console, the framebuffer the firmware sets up,
everything that runs from the initramfs, power-off and reboot, the SD
card (above), USB (the DWC2 controller, below: a keyboard, a mouse, a
stick) and, untried like the Pi 4's, Wi-Fi (the 3 B's CYW43438 or the
3 B+'s CYW43455 as `/dev/sdio`, driven by the `wifi` daemon: "Broadcom's
radios" in `docs/wifi.md`). What does not (yet): Ethernet (the LAN9514
behind the USB hub is a class driver to write). It has no RTC either: the
wall clock counts from the epoch at boot.

### USB: the DWC2 controller

The Pi 3's USB is Synopsys's DesignWare USB 2.0 OTG core
(`brcm,bcm2708-usb`, one root port, 8 host channels, interrupt GPU 9),
driven as a host by the `dwc2` module (`modules/dwc2`), a controller
driver of the USB core (`docs/usb.md`: the `UsbHcdOps` the core calls,
everything above it the same as on xHCI). It powers the controller
through the firmware (`rpi_firmware`, above) when that module is there,
resets the core, selects the UTMI+ PHY and host mode, sizes and flushes
the FIFOs and turns buffer DMA on. A transfer takes a channel: its
registers say the device, the endpoint, the packets and where in memory
(`HCDMA`, a bounce page of the module's, the Pi's DMA not seeing the
caches), the channel halts into its record (`HCINT`) and the handler
wakes the waiter; the stages of a control transfer, a NAKed interrupt
endpoint's retry (asked again after its interval from the core's thread,
`poll`) and the halves of a split transaction run in task context. A low-
or full-speed device behind a high-speed hub (every keyboard and mouse on
a real Pi 3, behind the LAN9514's hub) goes through split transactions
(`HCSPLT`: the hub's address and port; a start half, then the complete
half until the translator has the answer), one packet per transaction
set; QEMU's hub is full-speed and the root port takes it natively, so
the split path is not exercised by CI. Data toggles are kept per
endpoint, from the packets each transfer moved. Bulk goes through a
64 KiB bounce buffer, a control transfer's data stage is 1 KiB at most.

`cargo run -- rpi3` attaches QEMU's hub to the root port with the stick,
the mouse and (in a test boot) the serial adapter behind it, as the virt
boards have them on their xHCI; the `usb_disk`, `mouse`, `usb_keyboard`
and `usbdev` tests run in the Pi 3 CI boot.

`boot/rpi3/build.sh` (which `cargo run -- rpi3` runs) builds the boot
files into `target/rpi3`:

- `armstub8-myos.bin` (`boot/rpi3/armstub8.S`, myos's own): every core
  starts in it at address 0 and leaves at EL1, the level Limine boots a
  core without VHE at (it refuses EL2 there). Core 0 jumps to U-Boot;
  the others wait in the device tree's spin table (`cpu-release-addr`),
  where Limine starts them. It stays at EL2 as a minimal PSCI, for the
  kernel's power-off and reboot (`docs/power.md`): both reset the board
  through the power manager's watchdog, power-off with the reset status
  the firmware reads as "halt".
- `u-boot.bin`: U-Boot (`UBOOT_VERSION` in `boot/rpi3/versions.env`,
  `rpi_3_defconfig`), built with GCC's AArch64 cross compiler
  (`gcc-aarch64-linux-gnu`; a clang build crashes at run time). It finds
  `EFI/BOOT/BOOTAA64.EFI` on the first disk and starts it with the
  firmware's device tree.
- `bcm2710-rpi-3-b.dtb`: the Pi 3's device tree from the firmware
  release `FIRMWARE_TAG`. QEMU has no GPU firmware, so the launcher does
  its part: it fills the memory node in a copy (`qemu.dtb`), writes the
  tree's and U-Boot's addresses into a copy of the stub
  (`armstub8-qemu.bin`) and loads the three where the firmware would.
- `myos.img`: the same disk as `target/rpi4.img`, padded to a power of
  two for QEMU's SD card.

The kernel finds the board in the device tree like the Pi 4's. Its
interrupt controllers are Broadcom's, not a GIC
(`kernel/src/arch/aarch64/bcm2836.rs`): the per-core local controller
(`brcm,bcm2836-l1-intc`: the timers, the mailboxes the IPIs go through,
the peripherals' interrupt routed to core 0) in front of the BCM2835's
(`brcm,bcm2836-armctrl-ic`, the peripherals', whose `bank, number`
specifiers are numbered 32 + n for GPU interrupt n and 96 + n for basic
interrupt n). The kernel's console is the UART `stdout-path` names, the
mini UART (`brcm,bcm2835-aux-uart`).

### On a real Pi 3

On a FAT32-formatted SD card: `bootcode.bin`, `start.elf`, `fixup.dat`
and `bcm2710-rpi-3-b.dtb` from the
[firmware release](https://github.com/raspberrypi/firmware/tree/master/boot)
(`FIRMWARE_TAG`), `target/rpi3/armstub8-myos.bin`,
`target/rpi3/u-boot.bin`, and a `config.txt`:

```
arm_64bit=1
enable_uart=1
kernel=u-boot.bin
armstub=armstub8-myos.bin
```

The myos disk goes on a USB stick (`sudo dd if=target/rpi3/myos.img
of=/dev/sdX bs=4M conv=fsync`), which U-Boot boots when the SD card has
no `EFI/BOOT` of its own. myos itself then runs from the initramfs: it
sees the SD card (`/dev/mmcblk0`), not the stick (no USB driver yet), so
`/boot` and `/data` are not mounted. The console is the mini UART on
GPIO 14 and 15 (the same pins as the Pi 4's above), at 115200 8N1.
