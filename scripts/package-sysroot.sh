#!/usr/bin/env bash
# Package the prebuilt myos sysroot for offline / CI cache distribution.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# shellcheck source=scripts/myos-sysroot-lib.sh
source "$ROOT/scripts/myos-sysroot-lib.sh"

"$ROOT/scripts/build-sysroot.sh"

version="$(cat "$MYOS_SYSROOT_VERSION")"
if command -v zstd >/dev/null 2>&1; then
  out="$ROOT/target/myos-sysroot-${version}.tar.zst"
  tar -C "$ROOT/target" -cf - myos-sysroot | zstd -T0 -19 -o "$out"
else
  out="$ROOT/target/myos-sysroot-${version}.tar.gz"
  tar -C "$ROOT/target" -czf "$out" myos-sysroot
  echo "note: zstd not found; packaged with gzip" >&2
fi
echo "packaged sysroot -> $out"
