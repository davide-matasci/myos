#!/usr/bin/env bash
# Cross-build st for the three arches against the X and font libraries of
# packages/x11-xft (x11-libs included): upstream st with one myos patch
# (st.c.myos.patch: TERMCAP stays set) and a compat header for what newlib's
# headers lack (myos_compat.h). Static, like everything; config.h is the
# stock config.def.h. Then the boot test st_smoke, which watches the
# framebuffer.
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
  "$cc" -O2 -Wall -Wextra "$HERE/st_smoke.c" -o "$ROOT/target/st-smoke-${arch}-unknown-none"
done

myos_st_version_hash >"$MYOS_ST_VERSION"
echo "st -> target/{st,st-smoke}-<arch>-unknown-none"
