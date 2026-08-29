#!/usr/bin/env bash
# Package the prebuilt myos sysroot for offline / CI cache distribution.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# shellcheck source=scripts/myos-sysroot-lib.sh
source "$ROOT/scripts/myos-sysroot-lib.sh"

"$ROOT/scripts/build-sysroot.sh"

version="$(cat "$MYOS_SYSROOT_VERSION")"
out="$ROOT/target/myos-sysroot-${version}.tar.zst"

tar -C "$ROOT/target" -cf - myos-sysroot | zstd -T0 -19 -o "$out"
echo "packaged sysroot -> $out"
