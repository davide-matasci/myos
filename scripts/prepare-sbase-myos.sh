#!/usr/bin/env bash
# Prepare a myos build tree from fetched upstream sbase (patch echo, no vendored tools).
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
"$ROOT/scripts/fetch-sbase.sh"

SBASE="$ROOT/target/sbase-src"
WORK="$ROOT/target/sbase-myos-build"
PATCH="$ROOT/scripts/sbase-myos/echo.myos.patch"

mkdir -p "$WORK"
cp "$SBASE/echo.c" "$WORK/echo.c"
patch -d "$WORK" -p0 --forward --batch < "$PATCH"
echo "sbase myos tree -> $WORK (patched echo.c)"
