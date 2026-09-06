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
  # Same packing trick for tcc (workflow edits need `workflow` scope).
  for f in target/coreutils-tcc-*; do
    dest="target/tcc-${f#target/coreutils-tcc-}"
    if [[ ! -f "$dest" ]]; then
      cp "$f" "$dest"
      echo "restored $dest from $f"
    fi
  done
}


tcc_elves_ready() {
  [[ -f target/tcc-x86_64-unknown-myos \
     && -f target/tcc-aarch64-unknown-myos \
     && -f target/tcc-riscv64-unknown-myos ]]
}

tcc_libtcc1_in_newlib() {
  local arch
  for arch in x86_64 aarch64 riscv64; do
    [[ -s "target/newlib-${arch}/${arch}-unknown-myos/lib/libtcc1.a" ]] || return 1
  done
  return 0
}



ncurses_libs_ready() {
  [[ -f target/ncurses-x86_64/lib/libncurses.a \
     && -f target/ncurses-aarch64/lib/libncurses.a \
     && -f target/ncurses-riscv64/lib/libncurses.a ]]
}

vim_elves_ready() {
  [[ -f target/vim-x86_64-unknown-none \
     && -f target/vim-aarch64-unknown-none \
     && -f target/vim-riscv64-unknown-none ]]
}

rg_elves_ready() {
  [[ -f target/rg-x86_64-unknown-myos \
     && -f target/rg-aarch64-unknown-myos \
     && -f target/rg-riscv64-unknown-myos ]]
}

rebuild_kernels() {
  # Hash-gated: same script as the CI "Build kernel..." step. No-op when
  # inputs/artifacts already match target/.myos-ci-kernel-version.
  echo "==> rebuild_kernels via scripts/ci-build-kernels.sh"
  ./scripts/ci-build-kernels.sh
}

if [[ -x target/debug/myos && -f target/bios.img \
     && -f target/aarch64-unknown-none-softfloat/debug/kernel \
     && -f target/riscv64imac-unknown-none-elf/debug/kernel ]]; then
  echo "CI artifacts ready: $(ls -lh target/debug/myos target/bios.img)"
  echo "prebuilt kernels: $(ls -lh target/aarch64-unknown-none-softfloat/debug/kernel target/riscv64imac-unknown-none-elf/debug/kernel)"
  touch target/.myos-ci-prebuilt-kernels
  restore_packed_rg_elves
  need_rebuild=0
  if ! rg_elves_ready; then
    echo "==> rg ELF(s) missing after restore; building ripgrep"
    ./ports/ripgrep/build.sh
    need_rebuild=1
  else
    echo "rg ELFs present: $(ls -lh target/rg-*-unknown-myos)"
  fi
  if ! tcc_elves_ready; then
    echo "==> tcc ELF(s) missing after restore; building tcc"
    ./ports/tcc/build.sh
    need_rebuild=1
  else
    echo "tcc ELFs present: $(ls -lh target/tcc-*-unknown-myos)"
  fi
  # Hosted tcc -o needs libtcc1.a inside target/newlib-*/.../lib (embedded at
  # /lib/newlib/lib). GHCR newlib does not carry it; tcc build/pack_aliases does.
  if ! tcc_libtcc1_in_newlib; then
    echo "==> libtcc1.a missing from newlib prefixes; running ports/tcc/build.sh"
    ./ports/tcc/build.sh
    need_rebuild=1
  fi
  if ! ncurses_libs_ready; then
    echo "==> ncurses lib(s) missing after restore; building ncurses"
    ./ports/ncurses/build.sh
  else
    echo "ncurses libs present: $(ls -lh target/ncurses-*/lib/libncurses.a)"
  fi
  if ! vim_elves_ready; then
    echo "==> vim ELF(s) missing after restore; building vim"
    ./ports/vim/build.sh
    need_rebuild=1
  else
    echo "vim ELFs present: $(ls -lh target/vim-*-unknown-none)"
  fi
  if ((need_rebuild)); then
    rebuild_kernels
  fi
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
./ports/ubase/build.sh
./ports/coreutils/build-uutils.sh
./ports/ripgrep/build.sh
./ports/tcc/build.sh
./ports/ncurses/build.sh
./ports/vim/build.sh
./ports/curl/build.sh

rebuild_kernels

test -x target/debug/myos
test -f target/bios.img
echo "Rebuild complete: $(ls -lh target/debug/myos target/bios.img)"