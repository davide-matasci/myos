#!/usr/bin/env bash
# Cross-build dwm for the three arches against the X libraries of
# packages/x11-libs, with the myos patch (core-fonts.myos.patch: core X
# fonts instead of Xft, so the server's built-in `fixed` draws the bar;
# children reaped in a SIGCHLD handler, myos has no SA_NOCLDWAIT). Static,
# like everything; config.h is the stock config.def.h. Then the boot test
# dwm_smoke, which watches the framebuffer.
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$HERE/../.." && pwd)"
# shellcheck source=scripts/myos-c-userspace-lib.sh
source "$ROOT/scripts/myos-c-userspace-lib.sh"
# shellcheck source=packages/dwm/versions.env
source "$HERE/versions.env"

if myos_dwm_is_current; then
  echo "dwm up to date"
  exit 0
fi

"$HERE/fetch.sh"
"$ROOT/toolchain/newlib/build.sh"
"$ROOT/packages/x11-libs/build.sh"
export PATH="$ROOT/target/newlib-bin:$PATH"

SRC="$ROOT/target/dwm-src/dwm-$DWM_VERSION"
WORK="$ROOT/target/dwm-build"
X11="$ROOT/packages/x11-libs"

rm -rf "$WORK"
mkdir -p "$WORK"
cp -a "$SRC" "$WORK/dwm"
patch -d "$WORK/dwm" -p1 --forward --batch -s < "$HERE/core-fonts.myos.patch"
cp "$WORK/dwm/config.def.h" "$WORK/dwm/config.h"

for arch in x86_64 aarch64 riscv64; do
  echo "==> dwm ($arch)"
  cc="$WORK/$arch-cc"
  stage="$ROOT/target/x11-libs-$arch/lib/x11"
  myos_write_cross_cc "$arch" "$cc" -I"$X11/include" -include "$X11/myos_compat.h"
  extra=()
  [[ "$arch" == riscv64 ]] && extra=("$HERE/riscv64-sf-arith.c")
  # dwm's config.mk flags, without Xinerama and Xft.
  (cd "$WORK/dwm" && "$cc" -std=c99 -pedantic -Wall -Wno-deprecated-declarations -Os \
    -D_DEFAULT_SOURCE -D_BSD_SOURCE -D_XOPEN_SOURCE=700L -DVERSION=\"$DWM_VERSION\" \
    -I"$stage/include" dwm.c drw.c util.c "${extra[@]+"${extra[@]}"}" \
    -o "$ROOT/target/dwm-${arch}-unknown-none" -L"$stage/lib" -lX11 -lxcb -lXau)
  "$cc" -O2 -Wall -Wextra "$HERE/dwm_smoke.c" -o "$ROOT/target/dwm-smoke-${arch}-unknown-none"
done

myos_dwm_version_hash >"$MYOS_DWM_VERSION"
echo "dwm -> target/{dwm,dwm-smoke}-<arch>-unknown-none"
