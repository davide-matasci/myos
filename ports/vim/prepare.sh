#!/usr/bin/env bash
# Prepare a myos build tree from fetched upstream Vim (config.h + stubs + patches).
#
# Config choice: hand-written ports/vim/config.h for FEAT_TINY freestanding
# (documented in that file). Host configure is NOT used — it enables
# terminfo/ncurses and Linux-only APIs myos lacks.
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$HERE/../.." && pwd)"
"$ROOT/ports/vim/fetch.sh"

VIM="$ROOT/target/vim-src"
WORK="$ROOT/target/vim-myos-build"
MYOS="$ROOT/ports/vim"

rm -rf "$WORK"
mkdir -p "$WORK"

# Only need the src tree (+ runtime runtime/doc bits unused at link time).
rsync -a \
  --exclude='.git' \
  --exclude='src/testdir' \
  --exclude='src/xxd' \
  --exclude='src/GvimExt' \
  --exclude='nsis' \
  --exclude='ci' \
  --exclude='.github' \
  "$VIM/" "$WORK/"

mkdir -p "$WORK/src/auto"
cp "$MYOS/config.h" "$WORK/src/auto/config.h"
cp "$MYOS/osdef.h" "$WORK/src/auto/osdef.h"
cp "$MYOS/pathdef.c" "$WORK/src/auto/pathdef.c"
cp "$MYOS/myos_stubs.c" "$WORK/src/myos_stubs.c"
cp "$MYOS/myos_compat.h" "$WORK/src/myos_compat.h"

# Apply ordered myos patches when present.
shopt -s nullglob
for p in "$MYOS"/*.myos.patch; do
  echo "apply $(basename "$p")"
  patch -d "$WORK" -p1 --forward --batch < "$p"
done

echo "vim myos tree -> $WORK"
