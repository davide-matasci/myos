#!/usr/bin/env bash
# Assert that ci-build.tar (already extracted) has everything boot / boot-mini
# need to run QEMU. NO compile / rebuild / registry / toolchain install.
#
# Boot jobs must call this after `tar -xf ci-build.tar` and must not fall back
# to scripts/ci-restore-or-build.sh. Missing bits = fail the build job's pack.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

# Pack-alias restore only (cp). Historical ci-build.tar may ship coreutils-*
# aliases instead of canonical names; never compile here.
restore_pack_aliases() {
  shopt -s nullglob
  local f dest bin
  for f in target/coreutils-rg-*; do
    dest="target/rg-${f#target/coreutils-rg-}"
    if [[ ! -f "$dest" ]]; then cp "$f" "$dest"; fi
  done
  for f in target/coreutils-tcc-*; do
    dest="target/tcc-${f#target/coreutils-tcc-}"
    if [[ ! -f "$dest" ]]; then cp "$f" "$dest"; fi
  done
  for f in target/coreutils-c-socket_smoke-*; do
    dest="target/c-socket_smoke-${f#target/coreutils-c-socket_smoke-}"
    if [[ ! -f "$dest" ]]; then cp "$f" "$dest"; fi
  done
  for f in target/coreutils-curl-*; do
    dest="target/curl-${f#target/coreutils-curl-}"
    if [[ ! -f "$dest" ]]; then cp "$f" "$dest"; fi
  done
  for bin in dropbear dbclient dropbearkey; do
    for f in target/coreutils-${bin}-*; do
      dest="target/${bin}-${f#target/coreutils-${bin}-}"
      if [[ ! -f "$dest" ]]; then cp "$f" "$dest"; fi
    done
  done
  for f in target/coreutils-pty-smoke-*; do
    dest="target/pty-smoke-${f#target/coreutils-pty-smoke-}"
    if [[ ! -f "$dest" ]]; then cp "$f" "$dest"; fi
  done
  for f in target/coreutils-urandom-smoke-*; do
    dest="target/urandom-smoke-${f#target/coreutils-urandom-smoke-}"
    if [[ ! -f "$dest" ]]; then cp "$f" "$dest"; fi
  done
  if [[ -f target/coreutils-cacert.pem && ! -f target/cacert.pem ]]; then
    cp target/coreutils-cacert.pem target/cacert.pem
  elif [[ -f target/cacert.pem && ! -f target/coreutils-cacert.pem ]]; then
    cp target/cacert.pem target/coreutils-cacert.pem
  fi
}

have() { [[ -e "$1" ]]; }

# Require canon, or its coreutils-* pack alias when provided as $2.
require() {
  local path="$1"
  local alias="${2:-}"
  if have "$path"; then
    return 0
  fi
  if [[ -n "$alias" ]] && have "$alias"; then
    return 0
  fi
  if [[ -n "$alias" ]]; then
    echo "::error::required CI artifact missing: $path (also tried $alias)"
  else
    echo "::error::required CI artifact missing: $path"
  fi
  return 1
}

restore_pack_aliases

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

for arch in x86_64 aarch64 riscv64; do
  none="unknown-none"
  require "target/c-socket_smoke-${arch}-${none}" \
    "target/coreutils-c-socket_smoke-${arch}-${none}" || missing=1
  require "target/curl-${arch}-${none}" \
    "target/coreutils-curl-${arch}-${none}" || missing=1
  require "target/dropbear-${arch}-${none}" \
    "target/coreutils-dropbear-${arch}-${none}" || missing=1
  require "target/dbclient-${arch}-${none}" \
    "target/coreutils-dbclient-${arch}-${none}" || missing=1
  require "target/dropbearkey-${arch}-${none}" \
    "target/coreutils-dropbearkey-${arch}-${none}" || missing=1
  require "target/pty-smoke-${arch}-${none}" \
    "target/coreutils-pty-smoke-${arch}-${none}" || missing=1
  require "target/urandom-smoke-${arch}-${none}" \
    "target/coreutils-urandom-smoke-${arch}-${none}" || missing=1
  require "target/tcp-listen-smoke-${arch}-${none}" || missing=1
done

require target/cacert.pem target/coreutils-cacert.pem || missing=1

# os-test: aarch64/riscv host myos re-packs initramfs under SKIP_KERNEL_REBUILD.
require target/os-test-embed/basic/ctype/isalnum.c || missing=1
for arch in x86_64 aarch64 riscv64; do
  require "target/os-test-prebuilt/${arch}/basic/arpa_inet/htons" || missing=1
done

if [[ "$missing" -ne 0 ]]; then
  echo "::error::ci-build.tar is incomplete for boot/boot-mini."
  echo "::error::The build job must pack these via scripts/ci-build-kernels.sh --print-members"
  echo "::error::(+ port globs). Boot jobs do not rebuild — fix the pack list, not boot."
  echo "target/ listing (myos / images / kernels / smokes):"
  ls -la target/debug/myos target/bios.img target/uefi.img \
    target/aarch64-unknown-none-softfloat/debug/kernel \
    target/riscv64imac-unknown-none-elf/debug/kernel \
    target/hello-* target/ok-* target/curl-* target/dropbear-* \
    target/pty-smoke-* target/urandom-smoke-* target/tcp-listen-smoke-* \
    target/cacert.pem target/coreutils-* 2>&1 | head -200 || true
  exit 1
fi

mkdir -p target
touch target/.myos-ci-prebuilt-kernels
echo "CI artifacts ready for QEMU (extract-only; no rebuild):"
ls -lh target/debug/myos target/bios.img target/uefi.img \
  target/aarch64-unknown-none-softfloat/debug/kernel \
  target/riscv64imac-unknown-none-elf/debug/kernel
