#!/usr/bin/env bash
# Prepare a myos build tree from fetched upstream Lynx.
#
# Config: hand-written packages/lynx/lynx_cfg.h (not host ./configure).
# SSL: lynx's own OpenSSL path, over LibreSSL (USE_OPENSSL_INCL).
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$HERE/../.." && pwd)"
"$HERE/fetch.sh"

SRC="$ROOT/target/lynx-src"
WORK="$ROOT/target/lynx-myos-build"
MYOS="$HERE"

rm -rf "$WORK"
mkdir -p "$WORK"
rsync -a \
  --exclude='.git' \
  --exclude='test' \
  --exclude='PACKAGE' \
  --exclude='BUILD' \
  --exclude='po' \
  "$SRC/" "$WORK/"

# Hand-config + help/cfg stubs at tree root (included as <lynx_cfg.h>).
cp "$MYOS/lynx_cfg.h" "$WORK/lynx_cfg.h"
cp "$MYOS/cfg_defs.h" "$WORK/cfg_defs.h"
cp "$MYOS/LYHelp.h" "$WORK/LYHelp.h"
cp "$MYOS/myos_compat.h" "$WORK/myos_compat.h"
cp "$MYOS/myos_stubs.c" "$WORK/src/myos_stubs.c"

# Host-build makeuctb + charset tables (needed by UCdomap.c).
CHR="$WORK/src/chrtrans"
if [[ ! -f "$CHR/iso01_uni.h" ]]; then
  echo "==> host makeuctb + unicode tables"
  HOSTCC="${HOSTCC:-gcc}"
  "$HOSTCC" -O2 -I"$WORK" -I"$WORK/src" -I"$WORK/src/chrtrans" \
    -I"$WORK/WWW/Library/Implementation" \
    -o "$CHR/makeuctb" "$CHR/makeuctb.c"
  shopt -s nullglob
  for tbl in "$CHR"/*_uni.tbl "$CHR"/*_suni.tbl; do
    [[ -f "$tbl" ]] || continue
    base="$(basename "$tbl" .tbl)"
    "$CHR/makeuctb" "$tbl" >"$CHR/${base}.h"
  done
fi

# Apply ordered myos patches when present.
shopt -s nullglob
for p in "$MYOS"/*.myos.patch; do
  echo "apply $(basename "$p")"
  patch -d "$WORK" -p1 --forward --batch <"$p"
done

python3 "$MYOS/patch_lycurses_opaque.py" "$WORK/src/LYCurses.h"

echo "lynx myos tree -> $WORK"
