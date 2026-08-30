#!/usr/bin/env bash
# Cross-compile C smoke ELFs with newlib + myos libgloss.
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
"$ROOT/scripts/build-newlib.sh"
export PATH="$ROOT/target/newlib-bin:$PATH"

link_prog() {
  local name="$1"
  local src="$2"
  local arch="$3"
  local triple="${arch}-unknown-myos"
  local out="$ROOT/target/c-${name}-${arch}-unknown-none"
  local prefix="$ROOT/target/newlib-${arch}"
  local inc="$prefix/${triple}/include"
  local lib="$prefix/${triple}/lib"
  local obj="$ROOT/target/c-${name}-${arch}.o"
  local cc="${triple}-cc"

  echo "==> c-${name} ($triple / newlib)"
  "$cc" -ffreestanding -fPIC -O2 -isystem "$inc" -c "$src" -o "$obj"

  if [[ "$arch" == "aarch64" ]]; then
    aarch64-linux-gnu-ld -pie --no-dynamic-linker -o "$out" \
      --entry=_start -z max-page-size=4096 \
      "$lib/crt0.o" "$obj" -L"$lib" --start-group -lc -lgloss -lg --end-group
  else
    ld.lld -pie --no-dynamic-linker -o "$out" \
      --entry=_start -z max-page-size=4096 \
      "$lib/crt0.o" "$obj" -L"$lib" --start-group -lc -lgloss -lg --end-group
  fi
  echo "c-${name} -> $out"
}

for arch in x86_64 aarch64; do
  link_prog hello "$ROOT/c/hello.c" "$arch"
done
