#!/usr/bin/env bash
# Pack ci-build.tar for boot/boot-mini (fail if incomplete).
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"
shopt -s nullglob
# Canonical kernel/image/hello/ok/curl/dropbear/smoke/cacert list —
# same members GHCR kernels packages and boot assert require.
mapfile -t kernel_members < <(./scripts/ci-build-kernels.sh --print-members)
files=(
  "${kernel_members[@]}"
  target/debug/build/myos-*/out
  target/hello-*
  target/ok-*
  target/std-*
  target/c-hello-*
  target/c-socket_smoke-*
  target/tcp-listen-smoke-*
  target/pty-smoke-*
  target/urandom-smoke-*
  target/curl-*
  target/sbase-*
  target/ubase-*
  target/oksh-*
  target/make-*
  target/uutils-*
  target/coreutils-*
  target/rg-*
  target/tcc-*
  target/vim-*
  target/lynx-*
  target/lua-*
  target/ncurses-*
  target/zlib-*
  target/git-*
  target/libncurses-*
  target/pcre2-x86_64
  target/pcre2-aarch64
  target/pcre2-riscv64
  target/.myos-newlib-version
  target/newlib-x86_64
  target/newlib-aarch64
  target/newlib-riscv64
  target/.myos-c-hello-version
  target/.myos-sbase-version
  target/.myos-ubase-version
  target/.myos-oksh-version
  target/.myos-make-version
  target/.myos-coreutils-version
  target/.myos-ripgrep-version
  target/.myos-tcc-version
  target/.myos-vim-version
  target/.myos-lynx-version
  target/.myos-lua-version
  target/.myos-ncurses-version
  target/.myos-zlib-version
  target/.myos-git-version
  target/.myos-std-hello-version
  target/.myos-dropbear-version
  target/.myos-os-test-version
  target/.myos-ci-kernel-version
  # Exact dirs — never glob target/os-test-* (matches -src / -prebuilt-obj).
  target/os-test-embed
  target/os-test-prebuilt
  target/limine-v*
)
# Exact ELF names only — never glob target/dropbear-* (matches
# dropbear-*-src / dropbear-myos-build trees).
for arch in x86_64 aarch64 riscv64; do
  for bin in dropbear dbclient dropbearkey; do
    files+=("target/${bin}-${arch}-unknown-none")
    files+=("target/coreutils-${bin}-${arch}-unknown-none")
  done
done
if [[ -f target/cacert.pem ]]; then
  files+=(target/cacert.pem)
fi
if [[ -f target/virt.dtb ]]; then
  files+=(target/virt.dtb)
fi
if [[ -d target/ovmf ]]; then
  files+=(target/ovmf)
fi
# Dedup while keeping only paths that exist (nullglob already dropped
# empty globs; exact print-members paths may be absent → required check).
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
  target/hello-x86_64-unknown-none
  target/hello-aarch64-unknown-none-softfloat
  target/hello-riscv64imac-unknown-none-elf
  target/ok-x86_64-unknown-none
  target/ok-aarch64-unknown-none-softfloat
  target/ok-riscv64imac-unknown-none-elf
  target/tcp-listen-smoke-x86_64-unknown-none
  target/tcp-listen-smoke-aarch64-unknown-none
  target/tcp-listen-smoke-riscv64-unknown-none
)
# Boot-critical ports: accept canon or coreutils-* alias.
require_one() {
  local c="$1" a="$2"
  if [[ -e "$c" || -e "$a" ]]; then return 0; fi
  echo "::error::required CI artifact missing: $c (also tried $a)"
  return 1
}
missing=0
for f in "${required[@]}"; do
  if [[ ! -e "$f" ]]; then
    echo "::error::required CI artifact missing: $f"
    missing=1
  fi
done
for arch in x86_64 aarch64 riscv64; do
  n=unknown-none
  require_one "target/c-socket_smoke-${arch}-${n}" "target/coreutils-c-socket_smoke-${arch}-${n}" || missing=1
  require_one "target/curl-${arch}-${n}" "target/coreutils-curl-${arch}-${n}" || missing=1
  require_one "target/dropbear-${arch}-${n}" "target/coreutils-dropbear-${arch}-${n}" || missing=1
  require_one "target/dbclient-${arch}-${n}" "target/coreutils-dbclient-${arch}-${n}" || missing=1
  require_one "target/dropbearkey-${arch}-${n}" "target/coreutils-dropbearkey-${arch}-${n}" || missing=1
  require_one "target/pty-smoke-${arch}-${n}" "target/coreutils-pty-smoke-${arch}-${n}" || missing=1
  require_one "target/urandom-smoke-${arch}-${n}" "target/coreutils-urandom-smoke-${arch}-${n}" || missing=1
done
require_one target/cacert.pem target/coreutils-cacert.pem || missing=1
# os-test embed + host-prebuilt smoke ELFs (initramfs always packs them).
if [[ ! -f target/os-test-embed/basic/ctype/isalnum.c ]]; then
  echo "::error::required CI artifact missing: target/os-test-embed (run ports/os-test/build.sh)"
  missing=1
fi
for arch in x86_64 aarch64 riscv64; do
  m="target/os-test-prebuilt/${arch}/basic/arpa_inet/htons"
  if [[ ! -f "$m" ]]; then
    echo "::error::required CI artifact missing: $m"
    missing=1
  fi
done
if [[ "$missing" -ne 0 ]]; then
  echo "target/ listing (hello/ok/kernels/ports):"
  ls -la target/hello-* target/ok-* \
    target/aarch64-unknown-none-softfloat/debug/kernel \
    target/riscv64imac-unknown-none-elf/debug/kernel \
    target/bios.img target/uefi.img target/debug/myos \
    target/curl-* target/dropbear-* target/pty-smoke-* \
    target/urandom-smoke-* target/tcp-listen-smoke-* \
    target/cacert.pem target/coreutils-* 2>&1 || true
  exit 1
fi
tar -cf ci-build.tar "${uniq[@]}"
ls -lh ci-build.tar
echo "packed ${#uniq[@]} members (includes ci-build-kernels --print-members)"
