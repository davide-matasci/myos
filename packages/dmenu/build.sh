#!/usr/bin/env bash
# Cross-build dmenu and stest for the three arches against the X and font
# libraries of packages/x11-xft (x11-libs included): upstream dmenu,
# unpatched, with a compat header for what newlib's headers lack
# (myos_compat.h). Static, like everything; config.h is the stock
# config.def.h. Its scripts dmenu_run and dmenu_path are shipped as they are
# (the kernel runs #! scripts). Then the boot test dmenu_smoke, which
# watches the framebuffer.
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$HERE/../.." && pwd)"
# shellcheck source=scripts/myos-c-userspace-lib.sh
source "$ROOT/scripts/myos-c-userspace-lib.sh"
# shellcheck source=packages/dmenu/versions.env
source "$HERE/versions.env"

if myos_dmenu_is_current; then
  echo "dmenu up to date"
  exit 0
fi

"$HERE/fetch.sh"
"$ROOT/toolchain/newlib/build.sh"
"$ROOT/packages/x11-xft/build.sh"
export PATH="$ROOT/target/newlib-bin:$PATH"

SRC="$ROOT/target/dmenu-src/dmenu-$DMENU_VERSION"
WORK="$ROOT/target/dmenu-build"
X11="$ROOT/packages/x11-libs"
XFT="$ROOT/packages/x11-xft"

rm -rf "$WORK"
mkdir -p "$WORK"
cp -a "$SRC" "$WORK/dmenu"
cp "$WORK/dmenu/config.def.h" "$WORK/dmenu/config.h"

for arch in x86_64 aarch64 riscv64; do
  echo "==> dmenu ($arch)"
  cc="$WORK/$arch-cc"
  stage="$ROOT/target/x11-xft-$arch/lib/x11"
  myos_write_cross_cc "$arch" "$cc" -I"$X11/include" -include "$X11/myos_compat.h" \
    -include "$XFT/myos_compat.h" -include "$HERE/myos_compat.h"
  # dmenu's config.mk flags, without Xinerama.
  flags=(-std=c99 -pedantic -Wall -Os -D_DEFAULT_SOURCE -D_BSD_SOURCE -D_XOPEN_SOURCE=700
    -D_POSIX_C_SOURCE=200809L -DVERSION=\"$DMENU_VERSION\")
  (cd "$WORK/dmenu" && "$cc" "${flags[@]}" -I"$stage/include" -I"$stage/include/freetype2" \
    dmenu.c drw.c util.c -o "$ROOT/target/dmenu-${arch}-unknown-none" -L"$stage/lib" \
    -lXft -lXrender -lfontconfig -lexpat -lfreetype -lX11 -lxcb -lXau -lm)
  (cd "$WORK/dmenu" && "$cc" "${flags[@]}" stest.c -o "$ROOT/target/stest-${arch}-unknown-none")
  "$cc" -O2 -Wall -Wextra "$HERE/dmenu_smoke.c" -o "$ROOT/target/dmenu-smoke-${arch}-unknown-none"
done
cp "$WORK/dmenu/dmenu_run" "$WORK/dmenu/dmenu_path" "$ROOT/target/"

myos_dmenu_version_hash >"$MYOS_DMENU_VERSION"
echo "dmenu -> target/{dmenu,stest,dmenu-smoke}-<arch>-unknown-none, target/dmenu_{run,path}"
