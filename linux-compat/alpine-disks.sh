#!/usr/bin/env bash
# alpine-disks.sh ARCH | --hash
#
# The boot tests' Alpine disk for ARCH, target/alpine-ARCH.img
# (docs/testing.md): one ext2 image with the roots the Linux layer tests
# mount from /dev/nvme2n1/data, each made by alpine-disk.sh (get-alpine -n
# on the host):
#   /rust     rustc (alpine_rustc)
#   /python   python3 (alpine_python, its sockets, asyncio)
#   /x11      xdpyinfo (alpine_x11, an X client on the native server)
# Alpine's latest-stable moves, so the roots are what it had when the disk
# was made; `--hash` prints the key the disk is cached under in CI (this
# script, get-alpine and its library): bump FIXTURE to make a fresh one.
# target/.myos-alpine-disks-ARCH-version holds the hash of the disk there;
# a stale one is made again.
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$HERE/.." && pwd)"
FIXTURE=1

hash() {
  {
    echo "$FIXTURE"
    sha256sum "$HERE/alpine-disks.sh" "$HERE/alpine-disk.sh" \
      "$ROOT/packages/get-alpine/get-alpine.c" "$ROOT/user/get-myos/pkgtools.c"
  } | sha256sum | cut -c1-16
}

case "${1:-}" in
  --hash) hash; exit 0 ;;
  x86_64 | aarch64 | riscv64) arch="$1" ;;
  *)
    echo "usage: $0 x86_64|aarch64|riscv64 | --hash" >&2
    exit 2
    ;;
esac
out="$ROOT/target/alpine-$arch.img"
stamp="$ROOT/target/.myos-alpine-disks-$arch-version"
if [[ -f "$out" && -f "$stamp" && "$(cat "$stamp")" == "$(hash)" ]]; then
  exit 0
fi
rm -f "$out" "$stamp"
"$HERE/alpine-disk.sh" "$arch" "$out" rust=rust python=python3 x11=xdpyinfo
hash > "$stamp"
