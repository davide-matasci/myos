#!/usr/bin/env bash
# Prepare a myos build tree from fetched upstream TinyCC (config.h, tccdefs, patches).
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$HERE/../.." && pwd)"
"$ROOT/ports/tcc/fetch.sh"

TCC="$ROOT/target/tcc-src"
WORK="$ROOT/target/tcc-myos-build"
MYOS="$ROOT/ports/tcc"

rm -rf "$WORK"
mkdir -p "$WORK"

rsync -a \
  --exclude='.git' \
  --exclude='tests' \
  --exclude='win32' \
  "$TCC/" "$WORK/"

cp "$MYOS/config.h" "$WORK/config.h"

patch -d "$WORK" -p0 --forward --batch < "$MYOS/tccrun.myos.patch"
patch -d "$WORK" -p0 --forward --batch < "$MYOS/tcc-hosted-link.myos.patch"

# tccdefs_.h is a C string table generated from include/tccdefs.h (CONFIG_TCC_PREDEFS).
HOSTCC="${HOSTCC:-cc}"
"$HOSTCC" -DC2STR "$WORK/conftest.c" -o "$WORK/c2str"
(cd "$WORK" && ./c2str include/tccdefs.h tccdefs_.h)
rm -f "$WORK/c2str"

echo "tinycc myos tree -> $WORK"
