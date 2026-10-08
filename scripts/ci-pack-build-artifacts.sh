#!/usr/bin/env bash
# Pack ci-build.tar.zst for boot/boot-mini (fail if incomplete).
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"
shopt -s nullglob
# The kernels, the images and the host harness (what the kernels registry
# package holds), the kernel modules, and every file the ports ship
# (scripts/ports.sh --all-files all: a boot job re-packs the aarch64 and
# riscv64 initramfs from the image ports' files and packs the packages from
# all of them; a manifest's ELFs included). Never a `-src` / `-build` tree.
mapfile -t kernel_members < <(./scripts/ci-build-kernels.sh --print-members)
mapfile -t image_files < <(./scripts/ports.sh --all-files all)
# The ports' stamps and outputs too: the ISO job runs `cargo build` on the
# extracted tar, and build.rs would otherwise rebuild a port whose ready
# file (a library's prefix) is not an image file.
mapfile -t port_outputs < <(./scripts/ports.sh --all-outputs all)
files=(
  "${kernel_members[@]}"
  "${image_files[@]}"
  "${port_outputs[@]}"
  target/debug/build/myos-*/out
  target/hello-*
  target/console-*
  target/pci_enum-*
  target/acpi-*
  target/virtio_blk-*
  target/nvme-*
  target/xhci-*
  target/usb_hub-*
  target/usb_storage-*
  target/virtio_net-*
  target/netfs-*
  target/fat-*
  target/ext2-*
  target/linux-*
  target/.myos-ci-kernel-version
  target/limine-v*
)
if [[ -f target/virt.dtb ]]; then
  files+=(target/virt.dtb)
fi
if [[ -d target/ovmf ]]; then
  files+=(target/ovmf)
fi
# Dedup while keeping only paths that exist (nullglob already dropped
# empty globs; exact paths may be absent → required check below).
declare -A seen=()
uniq=()
for f in "${files[@]}"; do
  [[ -e "$f" ]] || continue
  [[ -n "${seen[$f]:-}" ]] && continue
  seen[$f]=1
  uniq+=("$f")
done
required=(
  target/debug/myos
  target/bios.img
  target/uefi.img
  target/aarch64-unknown-none-softfloat/debug/kernel
  target/riscv64imac-unknown-none-elf/debug/kernel
  "${image_files[@]}"
)
for m in console hello pci_enum acpi virtio_blk nvme xhci usb_hub usb_storage virtio_net netfs fat ext2 linux; do
  for t in x86_64-unknown-none aarch64-unknown-none-softfloat riscv64imac-unknown-none-elf; do
    required+=("target/${m}-${t}")
  done
done
for t in x86_64-unknown-none aarch64-unknown-none riscv64-unknown-none; do
  required+=("target/linux-launcher-${t}")
done
missing=0
for f in "${required[@]}"; do
  if [[ ! -e "$f" ]]; then
    echo "::error::required CI artifact missing: $f"
    missing=1
  fi
done
if [[ "$missing" -ne 0 ]]; then
  echo "target/ listing:"
  ls -la target/ 2>&1 | head -200 || true
  exit 1
fi
# zstd on every core: the artifact upload stores it as is (the default
# zip deflate of the 1.6 GB tar was the slowest part of the upload).
tar --sparse -I 'zstd -T0 -3' -cf ci-build.tar.zst "${uniq[@]}"
ls -lh ci-build.tar.zst
echo "packed ${#uniq[@]} members (includes ci-build-kernels --print-members)"
