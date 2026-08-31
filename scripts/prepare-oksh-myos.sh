#!/usr/bin/env bash
# Prepare a myos build tree from fetched upstream oksh (pconfig.h + patches).
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
"$ROOT/scripts/fetch-oksh.sh"

OKSH="$ROOT/target/oksh-src"
WORK="$ROOT/target/oksh-myos-build"
MYOS="$ROOT/scripts/oksh-myos"

rm -rf "$WORK"
mkdir -p "$WORK"

rsync -a \
  --exclude='.git' \
  --exclude='CVS' \
  --exclude='*.1' \
  "$OKSH/" "$WORK/"

cp "$MYOS/pconfig.h" "$WORK/pconfig.h"

patch_copy() {
  local base="$1"
  patch -d "$WORK" -p0 --forward --batch < "$MYOS/${base}.myos.patch"
}

patch_copy main
patch_copy jobs
patch_copy tty

echo "oksh myos tree -> $WORK"
