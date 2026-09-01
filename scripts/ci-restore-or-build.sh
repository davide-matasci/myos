#!/usr/bin/env bash
# Restore ci-build.tar from the build job, or rebuild when missing (PR artifact skip / quota).
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

if [[ -f ci-build.tar ]]; then
  echo "==> extracting ci-build.tar"
  tar -xf ci-build.tar
fi

restore_packed_rg_elves() {
  # Prefer real target/rg-* from ci-build.tar. Fall back to the old
  # target/coreutils-rg-* alias used before the workflow packed rg directly.
  shopt -s nullglob
  local f dest
  for f in target/coreutils-rg-*; do
    dest="target/rg-${f#target/coreutils-rg-}"
    if [[ ! -f "$dest" ]]; then
      cp "$f" "$dest"
      echo "restored $dest from $f"
    fi
  done
}

rg_elves_ready() {
  [[ -f target/rg-x86_64-unknown-myos \
     && -f target/rg-aarch64-unknown-myos \
     && -f target/rg-riscv64-unknown-myos ]]
}

rebuild_kernels() {
  echo "==> rebuilding kernels so include_bytes! picks up /c/rg"
  cargo clean -p myos
  cargo clean -p kernel --target x86_64-unknown-none
  cargo clean -p kernel --target aarch64-unknown-none-softfloat
  cargo clean -p kernel --target riscv64imac-unknown-none-elf
  cargo build
  cargo build -p kernel --target aarch64-unknown-none-softfloat
  cargo build -p kernel --target riscv64imac-unknown-none-elf
}

if [[ -x target/debug/myos && -f target/bios.img ]]; then
  echo "CI artifacts ready: $(ls -lh target/debug/myos target/bios.img)"
  restore_packed_rg_elves
  if rg_elves_ready; then
    echo "rg ELFs present: $(ls -lh target/rg-*-unknown-myos)"
    exit 0
  fi
  echo "==> rg ELF(s) missing after restore; building ripgrep and rebuilding kernels"
  ./ports/ripgrep/build.sh
  rebuild_kernels
  test -x target/debug/myos
  test -f target/bios.img
  exit 0
fi

echo "==> ci-build missing or incomplete; rebuilding (PR artifact skip or quota)"
chmod +x scripts/*.sh ports/*/*.sh toolchain/*/*.sh

if compgen -G "target/myos-sysroot-*.tar.zst" > /dev/null; then
  export MYOS_SYSROOT_TARBALL="$(ls target/myos-sysroot-*.tar.zst | head -1)"
fi
./toolchain/std/fetch-sysroot.sh
./toolchain/std/build-std-hello.sh
./scripts/build-c-hello.sh
./ports/sbase/build.sh
./ports/oksh/build.sh
./ports/coreutils/build-uutils.sh
./ports/ripgrep/build.sh

rebuild_kernels

test -x target/debug/myos
test -f target/bios.img
echo "Rebuild complete: $(ls -lh target/debug/myos target/bios.img)"
