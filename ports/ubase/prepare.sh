#!/usr/bin/env bash
# Prepare a myos build tree from fetched upstream ubase (patches, config.h).
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$HERE/../.." && pwd)"
"$ROOT/ports/ubase/fetch.sh"

UBASE="$ROOT/target/ubase-src"
WORK="$ROOT/target/ubase-myos-build"
MYOS="$ROOT/ports/ubase"

rm -rf "$WORK"
mkdir -p "$WORK"

rsync -a \
  --exclude='.git' \
  --exclude='*.1' \
  --exclude='*.8' \
  "$UBASE/" "$WORK/"

cp "$MYOS/config.h" "$WORK/config.h"

patch_copy() {
  local base="$1"
  patch -d "$WORK" -p0 --forward --batch < "$MYOS/${base}.myos.patch"
}

patch_copy getty
patch_copy login

echo "ubase myos tree -> $WORK"
