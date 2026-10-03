#!/usr/bin/env bash
# Pack ci-build.tar for boot/boot-mini (fail if incomplete).
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"
shopt -s nullglob
# The kernels, the images and the host harness (what the kernels registry
# package holds), the kernel modules, and every file the image ports ship
# (scripts/ports.sh --all-image-files: a boot job re-packs the aarch64 and
# riscv64 initramfs from them; a manifest's ELFs included). Never a `-src`
# / `-build` tree.
mapfile -t kernel_members < <(./scripts/ci-build-kernels.sh --print-members)
mapfile -t image_files < <(./scripts/ports.sh --all-image-files)
# The ports' stamps and outputs too: the ISO job runs `cargo build` on the
# extracted tar, and build.rs would otherwise rebuild a port whose ready
# file (a library's prefix) is not an image file.
mapfile -t port_outputs < <(./scripts/ports.sh --all-outputs)
files=(
  "${kernel_members[@]}"
  "${image_files[@]}"
  "${port_outputs[@]}"
  target/debug/build/myos-*/out
  target/hello-*
  target/console-*
  target/stubfs-*
  target/pci_enum-*
  target/acpi-*
  target/virtio_blk-*
  target/nvme-*
  target/virtio_net-*
  target/netfs-*
  target/fat-*
  target/ext2-*
  target/linux-*
  target/ok-*
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
for m in console stubfs hello pci_enum acpi virtio_blk nvme virtio_net netfs fat ext2 linux ok; do
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
tar -cf ci-build.tar "${uniq[@]}"
ls -lh ci-build.tar
echo "packed ${#uniq[@]} members (includes ci-build-kernels --print-members)"
