#!/usr/bin/env bash
# Cross-compile the tcp-listen boot-CI smoke ELF with newlib + myos libgloss.
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# shellcheck source=scripts/myos-c-userspace-lib.sh
source "$ROOT/scripts/myos-c-userspace-lib.sh"

"$ROOT/toolchain/newlib/build.sh"
export PATH="$ROOT/target/newlib-bin:$PATH"
myos_ensure_llvm_bin

for arch in x86_64 aarch64 riscv64; do
  triple="${arch}-unknown-myos"
  out="$ROOT/target/tcp-listen-smoke-${arch}-unknown-none"
  prefix="$ROOT/target/newlib-${arch}"
  inc="$prefix/${triple}/include"
  lib="$prefix/${triple}/lib"
  obj="$ROOT/target/tcp-listen-smoke-${arch}.o"
  cc="${triple}-cc"
  extra=()

  echo "==> tcp-listen-smoke ($triple)"
  "$cc" -ffreestanding -fPIC -O2 -isystem "$inc" -c "$ROOT/c/tcp_listen_smoke.c" -o "$obj"

  # soft-float long-double helpers (same as c-hello/lynx: libc dtoa/printf
  # pull long-double + double helpers). aarch64: trunctfdf2; riscv64: the
  # softfloat shim (already includes the TF trunc/extend set — adding
  # trunctfdf2.c there would collide with it).
  if [[ "$arch" == "aarch64" ]]; then
    tf="$ROOT/target/tcp-listen-smoke-${arch}-trunctfdf2.o"
    "$cc" -ffreestanding -fPIC -O2 -isystem "$inc" -c "$ROOT/ports/sbase/trunctfdf2.c" -o "$tf"
    extra+=("$tf")
  elif [[ "$arch" == "riscv64" ]]; then
    sf="$ROOT/target/tcp-listen-smoke-${arch}-softfloat.o"
    "$cc" -ffreestanding -fPIC -O2 -isystem "$inc" -c "$ROOT/ports/sbase/riscv64-softfloat.c" -o "$sf"
    extra+=("$sf")
  fi

  ld.lld -pie --no-dynamic-linker -o "$out" \
    --entry=_start -z max-page-size=4096 \
    "$lib/crt0.o" "$obj" "${extra[@]+"${extra[@]}"}" -L"$lib" \
    --start-group -lc -lgloss -lg --end-group
  echo "tcp-listen-smoke -> $out"
done
