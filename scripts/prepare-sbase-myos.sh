#!/usr/bin/env bash
# Prepare a myos build tree from fetched upstream sbase (patches, no vendored tools).
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
"$ROOT/scripts/fetch-sbase.sh"

SBASE="$ROOT/target/sbase-src"
WORK="$ROOT/target/sbase-myos-build"
MYOS="$ROOT/scripts/sbase-myos"

mkdir -p "$WORK"

patch_copy() {
  local base="$1"
  cp "$SBASE/${base}.c" "$WORK/${base}.c"
  patch -d "$WORK" -p0 --forward --batch < "$MYOS/${base}.myos.patch"
}

patch_copy echo
patch_copy basename
patch_copy dirname
patch_copy pwd
patch_copy ls

echo "sbase myos tree -> $WORK"
