#!/usr/bin/env bash
# Build libmyos-c.a for x86_64 and AArch64 (freestanding, host cross-compile).
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
INC="$ROOT/libc/include"
SRC="$ROOT/libc/src"
CC="${MYOS_C_CC:-clang}"

build_lib() {
  local arch="$1"
  local triple="$2"
  local crt="$3"
  local out="$ROOT/target/c-lib-${arch}"
  rm -rf "$out"
  mkdir -p "$out/obj"
  echo "==> libmyos-c ($triple)"
  for f in syscall unistd string stdio stdlib environ; do
    "$CC" --target="$triple" -nostdlib -nostdinc -ffreestanding -fPIC -O2 \
      -isystem "$INC" -c "$SRC/${f}.c" -o "$out/obj/${f}.o"
  done
  "$CC" --target="$triple" -c "$crt" -o "$out/obj/crt0.o"
  rm -f "$out/libmyos-c.a"
  ar rcs "$out/libmyos-c.a" "$out/obj"/*.o
  echo "libmyos-c -> $out/libmyos-c.a"
}

build_lib x86_64 x86_64-unknown-none "$SRC/crt/x86_64/crt0.S"
build_lib aarch64 aarch64-unknown-none "$SRC/crt/aarch64/crt0.S"
