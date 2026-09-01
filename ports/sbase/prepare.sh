#!/usr/bin/env bash
# Prepare a myos build tree from fetched upstream sbase (patches, generated sources).
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$HERE/../.." && pwd)"
"$ROOT/ports/sbase/fetch.sh"

SBASE="$ROOT/target/sbase-src"
WORK="$ROOT/target/sbase-myos-build"
MYOS="$ROOT/ports/sbase"

rm -rf "$WORK"
mkdir -p "$WORK"

rsync -a \
  --exclude='.git' \
  --exclude='tests' \
  --exclude='sbase-box' \
  "$SBASE/" "$WORK/"

patch_copy() {
  local base="$1"
  patch -d "$WORK" -p0 --forward --batch < "$MYOS/${base}.myos.patch"
}

patch_copy echo
patch_copy ls
patch_copy pwd
patch_copy touch

if [[ ! -f "$WORK/getconf.h" ]]; then
  (cd "$WORK" && sh scripts/getconf.sh >getconf.h)
fi

if [[ ! -f "$WORK/bc.c" ]]; then
  if command -v bison >/dev/null 2>&1; then
    bison -d -o "$WORK/bc.c" "$WORK/bc.y"
  elif command -v yacc >/dev/null 2>&1; then
    yacc -d -o "$WORK/bc.c" "$WORK/bc.y"
  else
    echo "warning: bison/yacc not found; skipping bc.c generation" >&2
  fi
fi

echo "sbase myos tree -> $WORK"
