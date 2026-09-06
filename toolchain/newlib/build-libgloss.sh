#!/usr/bin/env bash
# Build myos libgloss.a + crt0.o for one architecture.
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$HERE/../.." && pwd)"
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
mkdir -p "$out/obj" "$libdir" "$inc" "$inc/sys"
cp "$ROOT/toolchain/newlib/libgloss/myos/crt0-${arch}.S" "$PORT/crt0.S"
cp "$ROOT/toolchain/newlib/libgloss/myos/crti-${arch}.S" "$PORT/crti.S"
cp "$ROOT/toolchain/newlib/libgloss/myos/crtn-${arch}.S" "$PORT/crtn.S"
# termios.c needs <termios.h> in the sysroot before compile.
cp "$ROOT/toolchain/newlib/libgloss/myos/termios.h" "$inc/termios.h"

for f in myos_raw syscalls stubs posix_stubs misc_stubs more_stubs ioctl environ getline dirent cwd basename dirname time pwdgrp readlink mmap mount fd_path termios socket inet netdb pollselect; do
  "$CC" -ffreestanding -fPIC -O2 -I"$PORT" -isystem "$inc" \
    -c "$PORT/${f}.c" -o "$out/obj/${f}.o"
done
"$CC" -c "$PORT/crt0.S" -o "$out/obj/crt0.o"
"$CC" -c "$PORT/crti.S" -o "$out/obj/crti.o"
"$CC" -c "$PORT/crtn.S" -o "$out/obj/crtn.o"

# crt*.o are standalone CRT objects, not members of libgloss.a
ar rcs "$out/libgloss.a" "$out/obj"/*.o
ar d "$out/libgloss.a" crti.o crtn.o 2>/dev/null || true
cp "$out/libgloss.a" "$libdir/libgloss.a"
cp "$out/obj/crt0.o" "$libdir/crt0.o"
cp "$out/obj/crti.o" "$libdir/crti.o"
cp "$out/obj/crtn.o" "$libdir/crtn.o"
mkdir -p "$libdir/specs" "$inc/sys"
cp "$PORT/myos.specs" "$libdir/specs/myos.specs"
cp "$ROOT/toolchain/newlib/libgloss/myos/sys/dirent.h" "$inc/sys/dirent.h"
cp "$ROOT/toolchain/newlib/libgloss/myos/sys/sysmacros.h" "$inc/sys/sysmacros.h"
cp "$ROOT/toolchain/newlib/libgloss/myos/sys/myos_extra.h" "$inc/sys/myos_extra.h"
cp "$ROOT/toolchain/newlib/libgloss/myos/sys/ioctl.h" "$inc/sys/ioctl.h"
cp "$ROOT/toolchain/newlib/libgloss/myos/sys/socket.h" "$inc/sys/socket.h"
cp "$ROOT/toolchain/newlib/libgloss/myos/sys/utsname.h" "$inc/sys/utsname.h"
cp "$ROOT/toolchain/newlib/libgloss/myos/sys/mman.h" "$inc/sys/mman.h"
cp "$ROOT/toolchain/newlib/libgloss/myos/utmp.h" "$inc/utmp.h"
cp "$ROOT/toolchain/newlib/libgloss/myos/termios.h" "$inc/termios.h"
mkdir -p "$inc/arpa" "$inc/netinet"
cp "$ROOT/toolchain/newlib/libgloss/myos/arpa/inet.h" "$inc/arpa/inet.h"
cp "$ROOT/toolchain/newlib/libgloss/myos/netinet/in.h" "$inc/netinet/in.h"
cp "$ROOT/toolchain/newlib/libgloss/myos/netdb.h" "$inc/netdb.h"
cp "$ROOT/toolchain/newlib/libgloss/myos/poll.h" "$inc/poll.h"
echo "libgloss-myos -> $libdir/libgloss.a"
