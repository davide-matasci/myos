#!/usr/bin/env bash
# Build myos libgloss.a + crt0.o for one architecture.
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
arch="${1:?arch}"
prefix="${2:?prefix}"
triple="${arch}-unknown-myos"
NEWLIB_SRC="$ROOT/target/newlib-src"
PORT="$NEWLIB_SRC/libgloss/myos"
CC="${triple}-cc"
libdir="$prefix/${triple}/lib"
inc="$prefix/${triple}/include"
out="$ROOT/target/libgloss-myos-${arch}"

rm -rf "$out"
mkdir -p "$out/obj" "$libdir"
cp "$ROOT/newlib/libgloss/myos/crt0-${arch}.S" "$PORT/crt0.S"

for f in myos_raw syscalls stubs environ; do
  "$CC" -ffreestanding -fPIC -O2 -I"$PORT" -isystem "$inc" \
    -c "$PORT/${f}.c" -o "$out/obj/${f}.o"
done
"$CC" -c "$PORT/crt0.S" -o "$out/obj/crt0.o"

ar rcs "$out/libgloss.a" "$out/obj"/*.o
cp "$out/libgloss.a" "$libdir/libgloss.a"
cp "$out/obj/crt0.o" "$libdir/crt0.o"
mkdir -p "$libdir/specs"
cp "$PORT/myos.specs" "$libdir/specs/myos.specs"
echo "libgloss-myos -> $libdir/libgloss.a"
