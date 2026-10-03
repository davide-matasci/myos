#!/usr/bin/env bash
# alpine-disk.sh ARCH OUT PACKAGE...
#
# An ext2 image OUT whose /alpine is an Alpine Linux root for ARCH with
# PACKAGE... and their dependencies installed: what
#     get-alpine -r /disk/alpine PACKAGE...
# leaves on a disk, made on the host by get-alpine itself, built for the
# host. The full boot test attaches it to the guest (docs/testing.md):
# installing a toolchain in the emulated guest takes hours, mounting one
# takes seconds. Nothing is redone while OUT exists.
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$HERE/.." && pwd)"

if [[ $# -lt 3 ]]; then
  echo "usage: $0 x86_64|aarch64|riscv64 OUT PACKAGE..." >&2
  exit 2
fi
arch="$1"
out="$2"
shift 2
case "$arch" in
  x86_64 | aarch64 | riscv64) ;;
  *)
    echo "alpine-disk: no Alpine repository for $arch" >&2
    exit 2
    ;;
esac
if [[ -f "$out" ]]; then
  exit 0
fi

# get-alpine for the host, with the zlib port's sources (inflate only).
"$ROOT/ports/zlib/fetch.sh" > /dev/null
zsrc="$ROOT/target/zlib-src"
work="$ROOT/target/alpine-disk-$arch"
rm -rf "$work"
mkdir -p "$work/root"
cc -O2 -DALPINE_ARCH="\"$arch\"" -I"$ROOT/user/get-myos" -I"$zsrc" \
  "$ROOT/linux-compat/get-alpine.c" "$ROOT/user/get-myos/pkgtools.c" \
  "$zsrc"/{inflate,inftrees,inffast,zutil,adler32,crc32}.c \
  -o "$work/get-alpine"

echo "==> Alpine $arch root with $* (for $out)"
"$work/get-alpine" -r "$work/root/alpine" "$@"
# The same ext2 features as mkfs.ext2 (`-d` copies the tree in).
mke2fs -q -F -t ext2 -b 4096 -O ^resize_inode,^dir_index,^ext_attr \
  -d "$work/root" "$out.tmp" 2G
mv "$out.tmp" "$out"
rm -rf "$work"
