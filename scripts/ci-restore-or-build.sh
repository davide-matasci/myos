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
  # build-ripgrep also writes target/coreutils-rg-* so ci-build.tar (which
  # already packs target/coreutils-*) carries rg ELFs for boot-matrix rebuilds.
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

if [[ -x target/debug/myos && -f target/bios.img ]]; then
  echo "CI artifacts ready: $(ls -lh target/debug/myos target/bios.img)"
  restore_packed_rg_elves
  if [[ -f target/rg-x86_64-unknown-myos && -f target/rg-aarch64-unknown-myos && -f target/rg-riscv64-unknown-myos ]]; then
    exit 0
  fi
  echo "==> rg ELF(s) missing after restore; building ripgrep"
  ./scripts/build-ripgrep-myos.sh
  exit 0
fi

echo "==> ci-build missing or incomplete; rebuilding (PR artifact skip or quota)"
chmod +x scripts/*.sh

if compgen -G "target/myos-sysroot-*.tar.zst" > /dev/null; then
  export MYOS_SYSROOT_TARBALL="$(ls target/myos-sysroot-*.tar.zst | head -1)"
fi
./scripts/fetch-sysroot.sh
./scripts/build-std-hello.sh
./scripts/build-c-hello.sh
./scripts/build-sbase.sh
./scripts/build-oksh.sh
./scripts/build-uutils-myos.sh
./scripts/build-ripgrep-myos.sh

cargo clean -p myos
cargo clean -p kernel --target x86_64-unknown-none
cargo build
cargo build -p kernel --target aarch64-unknown-none-softfloat
cargo build -p kernel --target riscv64imac-unknown-none-elf

test -x target/debug/myos
test -f target/bios.img
echo "Rebuild complete: $(ls -lh target/debug/myos target/bios.img)"
