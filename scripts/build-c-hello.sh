#!/usr/bin/env bash
# Cross-compile small C smoke ELFs with libmyos-c (clang; optional tcc noted in README).
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
"$ROOT/scripts/build-c-libc.sh"
INC="$ROOT/libc/include"
CC="${MYOS_C_CC:-clang}"

link_prog() {
  local name="$1"
  local src="$2"
  local triple="$3"
  local arch="$4"
  local lib="$ROOT/target/c-lib-${arch}/libmyos-c.a"
  local out="$ROOT/target/c-${name}-${triple}"
  local obj="$ROOT/target/c-${name}-${arch}.o"
  echo "==> c-${name} ($triple)"
  "$CC" --target="$triple" -ffreestanding -nostdinc -fPIC -O2 -isystem "$INC" \
    -c "$src" -o "$obj"
  if [[ "$arch" == "aarch64" ]]; then
    aarch64-linux-gnu-ld -pie --no-dynamic-linker -o "$out" \
      --entry=_start -z max-page-size=4096 "$obj" "$lib"
  else
    ld.lld -pie --no-dynamic-linker -o "$out" \
      --entry=_start -z max-page-size=4096 "$obj" "$lib"
  fi
  echo "c-${name} -> $out"
}

for triple in x86_64-unknown-none aarch64-unknown-none riscv64-unknown-none; do
  arch="${triple%-unknown-none}"
  link_prog hello "$ROOT/c/hello.c" "$triple" "$arch"
  link_prog echo "$ROOT/c/echo.c" "$triple" "$arch"
done
