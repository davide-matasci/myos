#!/usr/bin/env bash
# alpine-disk.sh ARCH OUT [NAME=]PACKAGE[,PACKAGE...]...
#
# An ext2 image OUT whose /alpine is an Alpine Linux root for ARCH with
# PACKAGE... and their dependencies unpacked: what
#     get-alpine -n -r /mnt/alpine PACKAGE...
# leaves on a disk, made on the host by get-alpine itself, built for the
# host. -n, since the host has no Linux layer to run apk under: no install
# script or trigger runs, and apk's database in the root knows nothing (a
# root bootstrapped in the guest has both). `NAME=PACKAGE,...` makes the
# root /NAME instead, and several of them share the image (the boot
# tests' fixture, alpine-disks.sh). The boot test attaches it to the
# guest (docs/testing.md): installing a toolchain in the emulated guest
# takes hours, mounting one takes seconds. The image is as big as its
# content needs. Nothing is redone while OUT exists.
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$HERE/.." && pwd)"

if [[ $# -lt 3 ]]; then
  echo "usage: $0 x86_64|aarch64|riscv64 OUT [NAME=]PACKAGE[,PACKAGE...]..." >&2
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
  "$ROOT/packages/get-alpine/get-alpine.c" "$ROOT/user/get-myos/pkgtools.c" \
  "$zsrc"/{inflate,inftrees,inffast,zutil,adler32,crc32}.c \
  -o "$work/get-alpine"

for spec in "$@"; do
  name=alpine
  if [[ "$spec" == *=* ]]; then
    name="${spec%%=*}"
    spec="${spec#*=}"
  fi
  echo "==> Alpine $arch root /$name with ${spec//,/ } (for $out)"
  # shellcheck disable=SC2086
  "$work/get-alpine" -n -r "$work/root/$name" ${spec//,/ }
done
# The same ext2 features as mkfs.ext2 (`-d` copies the tree in); room for
# the content, the metadata and what the tests write (the layer's binds).
size="$(( $(du -sm "$work/root" | cut -f1) * 13 / 10 + 32 ))M"
mke2fs -q -F -t ext2 -b 4096 -O ^resize_inode,^dir_index,^ext_attr \
  -d "$work/root" "$out.tmp" "$size"
mv "$out.tmp" "$out"
rm -rf "$work"
