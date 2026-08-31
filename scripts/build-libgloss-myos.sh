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

mkdir -p "$libdir/specs" "$inc/sys"
cp "$ROOT/newlib/libgloss/myos/sys/termios.h" "$inc/sys/termios.h"
# newlib's resource.h has rusage but no rlimit (oksh ulimit).
cp "$ROOT/scripts/oksh-myos/sys/resource.h" "$inc/sys/resource.h"
cp "$ROOT/scripts/oksh-myos/sys/param.h" "$inc/sys/param.h"

for f in myos_raw syscalls stubs posix_stubs misc_stubs more_stubs environ getline dirent cwd basename dirname time pwdgrp readlink; do
  "$CC" -ffreestanding -fPIC -O2 -isystem "$inc" -I"$PORT" \
    -c "$PORT/${f}.c" -o "$out/obj/${f}.o"
done
"$CC" -c "$PORT/crt0.S" -o "$out/obj/crt0.o"

ar rcs "$out/libgloss.a" "$out/obj"/*.o
cp "$out/libgloss.a" "$libdir/libgloss.a"
cp "$out/obj/crt0.o" "$libdir/crt0.o"
cp "$PORT/myos.specs" "$libdir/specs/myos.specs"
cp "$ROOT/newlib/libgloss/myos/sys/dirent.h" "$inc/sys/dirent.h"
cp "$ROOT/newlib/libgloss/myos/sys/sysmacros.h" "$inc/sys/sysmacros.h"
cp "$ROOT/newlib/libgloss/myos/sys/myos_extra.h" "$inc/sys/myos_extra.h"
cp "$ROOT/newlib/libgloss/myos/sys/ioctl.h" "$inc/sys/ioctl.h"
cp "$ROOT/newlib/libgloss/myos/sys/socket.h" "$inc/sys/socket.h"
cp "$ROOT/newlib/libgloss/myos/sys/utsname.h" "$inc/sys/utsname.h"
cp "$ROOT/newlib/libgloss/myos/paths.h" "$inc/paths.h"
echo "libgloss-myos -> $libdir/libgloss.a"
