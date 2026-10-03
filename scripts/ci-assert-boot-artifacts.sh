#!/usr/bin/env bash
# Assert that ci-build.tar (already extracted) has everything boot / boot-mini
# need to run QEMU. NO compile / rebuild / registry / toolchain install.
#
# Boot jobs must call this after `tar -xf ci-build.tar`; building and packing
# belong in the build job (and the ports job). Missing bits =
# fail the build job's pack (scripts/ci-pack-build-artifacts.sh).
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

require() {
  if [[ -e "$1" ]]; then
    return 0
  fi
  echo "::error::required CI artifact missing: $1"
  return 1
}

missing=0
require target/debug/myos || missing=1
require target/bios.img || missing=1
require target/uefi.img || missing=1
require target/aarch64-unknown-none-softfloat/debug/kernel || missing=1
require target/riscv64imac-unknown-none-elf/debug/kernel || missing=1

# Limine modules / initramfs inputs that aarch64+riscv host myos re-packs
# when MYOS_SKIP_KERNEL_REBUILD=1 (bios/uefi images already embed them).
for arch_hello in \
  hello-x86_64-unknown-none \
  hello-aarch64-unknown-none-softfloat \
  hello-riscv64imac-unknown-none-elf \
  ok-x86_64-unknown-none \
  ok-aarch64-unknown-none-softfloat \
  ok-riscv64imac-unknown-none-elf; do
  require "target/${arch_hello}" || missing=1
done

# Every file the image ports ship, from their descriptors (scripts/ports.sh
# --all-image-files): the ELFs, manifests, data files and trees the initramfs
# is packed from.
while read -r f; do
  require "$f" || missing=1
done < <(./scripts/ports.sh --all-image-files)

if [[ "$missing" -ne 0 ]]; then
  echo "::error::ci-build.tar is incomplete for boot/boot-mini."
  echo "::error::The build job must pack these via scripts/ci-pack-build-artifacts.sh"
  echo "::error::(kernels --print-members + ports.sh --all-image-files). Boot jobs do not rebuild."
  echo "target/ listing:"
  ls -la target/ 2>&1 | head -200 || true
  exit 1
fi

mkdir -p target
touch target/.myos-ci-prebuilt-kernels
echo "CI artifacts ready for QEMU (extract-only; no rebuild):"
ls -lh target/debug/myos target/bios.img target/uefi.img \
  target/aarch64-unknown-none-softfloat/debug/kernel \
  target/riscv64imac-unknown-none-elf/debug/kernel
