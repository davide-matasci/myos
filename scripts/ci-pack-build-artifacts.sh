#!/bin/sh
# Pack ci-build.tar.zst for boot/boot-mini (fail if incomplete).
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
# The kernels, the images and the host harness (what the kernels registry
# package holds), the kernel modules, and every file the ports ship
# (scripts/ports.sh --all-files all: a boot job re-packs the aarch64 and
# riscv64 initramfs from the image ports' files and packs the packages from
# all of them; a manifest's ELFs included). Never a `-src` / `-build` tree.
kernel_members="$(./scripts/ci-build-kernels.sh --print-members)" || true
image_files="$(./scripts/ports.sh --all-files all)" || true
# The ports' stamps and outputs too: the ISO job runs `cargo build` on the
# extracted tar, and build.rs would otherwise rebuild a port whose ready
# file (a library's prefix) is not an image file.
port_outputs="$(./scripts/ports.sh --all-outputs all)" || true
# The Raspberry Pi 3's boot files (boot/rpi3/build.sh, the build job's
# step): the rpi3 boot needs no cross compiler then.
rpi3_files="
  target/rpi3/armstub8-myos.bin
  target/rpi3/armstub8.sha256
  target/rpi3/u-boot.bin
  target/rpi3/u-boot.version
  target/rpi3/bcm2710-rpi-3-b.dtb
"
virt_dtb=""
if [ -f target/virt.dtb ]; then
  virt_dtb=target/virt.dtb
fi
ovmf=""
if [ -d target/ovmf ]; then
  ovmf=target/ovmf
fi
# Dedup while keeping only paths that exist (a glob that matches nothing
# stays as it is and is dropped here; exact paths may be absent → required
# check below). The lists are split on purpose: no path has a blank.
# shellcheck disable=SC2086
uniq="$(
  for f in \
    $kernel_members \
    $image_files \
    $port_outputs \
    target/debug/build/myos-*/out \
    target/hello-* \
    target/console-* \
    target/pci_enum-* \
    target/acpi-* \
    target/rpi_firmware-* \
    target/virtio_blk-* \
    target/virtio_scsi-* \
    target/nvme-* \
    target/sdmmc-* \
    target/usb-* \
    target/xhci-* \
    target/ehci-* \
    target/uhci-* \
    target/ohci-* \
    target/dwc2-* \
    target/usb_hub-* \
    target/usb_storage-* \
    target/usb_hid-* \
    target/usb_audio-* \
    target/sound-* \
    target/virtio_sound-* \
    target/usbdev-* \
    target/pcidev-* \
    target/pl061-* \
    target/tap-* \
    target/virtio_net-* \
    target/netfs-* \
    target/fat-* \
    target/ext2-* \
    target/linux-* \
    target/.myos-ci-kernel-version \
    target/limine-v* \
    $virt_dtb \
    $rpi3_files \
    $ovmf
  do
    [ -e "$f" ] || continue
    printf '%s\n' "$f"
  done | awk '!seen[$0]++'
)"
required="
  target/debug/myos
  target/bios.img
  target/uefi.img
  target/aarch64-unknown-none-softfloat/debug/kernel
  target/riscv64imac-unknown-none-elf/debug/kernel
  $image_files
  $rpi3_files
"
for m in console hello pci_enum acpi rpi_firmware virtio_blk virtio_scsi nvme sdmmc usb xhci ehci uhci ohci dwc2 usb_hub usb_storage usb_hid usb_audio sound virtio_sound usbdev pcidev pl061 virtio_net tap netfs fat ext2 linux; do
  for t in x86_64-unknown-none aarch64-unknown-none-softfloat riscv64imac-unknown-none-elf; do
    required="$required target/${m}-${t}"
  done
done
for t in x86_64-unknown-none aarch64-unknown-none riscv64-unknown-none; do
  required="$required target/linux-launcher-${t}"
done
missing=0
# shellcheck disable=SC2086
for f in $required; do
  if [ ! -e "$f" ]; then
    echo "::error::required CI artifact missing: $f"
    missing=1
  fi
done
if [ "$missing" -ne 0 ]; then
  echo "target/ listing:"
  ls -la target/ 2>&1 | head -200 || true
  exit 1
fi
# zstd on every core: the artifact upload stores it as is (the default
# zip deflate of the 1.6 GB tar was the slowest part of the upload).
# shellcheck disable=SC2086
tar --sparse -I 'zstd -T0 -3' -cf ci-build.tar.zst $uniq
ls -lh ci-build.tar.zst
# shellcheck disable=SC2086
set -- $uniq
echo "packed $# members (includes ci-build-kernels --print-members)"
