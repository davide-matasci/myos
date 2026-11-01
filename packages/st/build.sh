#!/usr/bin/env bash
# Cross-build st for the three arches against the X and font libraries of
# packages/x11-xft (x11-libs included): upstream st with one myos patch
# (st.c.myos.patch: TERMCAP stays set) and a compat header for what newlib's
# headers lack (myos_compat.h). Static, like everything; config.h is the
# stock config.def.h with the font in points (below). Then the boot test
# st_smoke, which watches the framebuffer.
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$HERE/../.." && pwd)"
# shellcheck source=scripts/myos-c-userspace-lib.sh
source "$ROOT/scripts/myos-c-userspace-lib.sh"
# shellcheck source=packages/st/versions.env
source "$HERE/versions.env"

if myos_st_is_current; then
  echo "st up to date"
  exit 0
fi

"$HERE/fetch.sh"
"$ROOT/toolchain/newlib/build.sh"
"$ROOT/packages/x11-xft/build.sh"
export PATH="$ROOT/target/newlib-bin:$PATH"

SRC="$ROOT/target/st-src/st-$ST_VERSION"
WORK="$ROOT/target/st-build"
X11="$ROOT/packages/x11-libs"
XFT="$ROOT/packages/x11-xft"

rm -rf "$WORK"
mkdir -p "$WORK"
cp -a "$SRC" "$WORK/st"
patch -d "$WORK/st" -p1 --forward --batch -s < "$HERE/st.c.myos.patch"
cp "$WORK/st/config.def.h" "$WORK/st/config.h"
# The font in points, not pixels, so it follows the X server's DPI as dwm's
# and dmenu's do (startx raises it on a large screen, packages/tinyx): the
# stock pixelsize=12 stayed 12 pixels on a laptop's 1920x1080 panel. 11.5
# points is 12 pixels at the server's own 75 DPI, as before.
sed -i 's/:pixelsize=12:/:size=11.5:/' "$WORK/st/config.h"
grep -q '"Liberation Mono:size=11.5:' "$WORK/st/config.h" || {
  echo "st: config.def.h's font is no longer pixelsize=12" >&2
  exit 1
}

for arch in x86_64 aarch64 riscv64; do
  echo "==> st ($arch)"
  cc="$WORK/$arch-cc"
  stage="$ROOT/target/x11-xft-$arch/lib/x11"
  myos_write_cross_cc "$arch" "$cc" -I"$X11/include" -include "$X11/myos_compat.h" \
    -include "$XFT/myos_compat.h"
  # st's config.mk flags.
  (cd "$WORK/st" && "$cc" -std=c99 -pedantic -Wall -Wno-deprecated-declarations -Os \
    -DVERSION=\"$ST_VERSION\" -D_XOPEN_SOURCE=600 -include "$HERE/myos_compat.h" \
    -I"$stage/include" -I"$stage/include/freetype2" st.c x.c \
    -o "$ROOT/target/st-${arch}-unknown-none" -L"$stage/lib" \
    -lXft -lXrender -lfontconfig -lexpat -lfreetype -lX11 -lxcb -lXau -lm)
  "$cc" -O2 -Wall -Wextra -I"$stage/include" "$HERE/st_smoke.c" \
    -o "$ROOT/target/st-smoke-${arch}-unknown-none" -L"$stage/lib" -lX11 -lxcb -lXau
done

myos_st_version_hash >"$MYOS_ST_VERSION"
echo "st -> target/{st,st-smoke}-<arch>-unknown-none"
