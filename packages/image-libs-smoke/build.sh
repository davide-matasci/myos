#!/usr/bin/env bash
# Link the image libraries' smoke (smoke.c) for the three arches against
# mlibc, packages/image-libs' archives and x11-libs' mlibc build. A package
# of its own, not image-libs', so that a change to what it links and the
# libraries never compile against (the libc's C first) relinks it and
# rebuilds no library (docs/ports.md "Cache keys").
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$HERE/../.." && pwd)"
# shellcheck source=scripts/myos-c-userspace-lib.sh
source "$ROOT/scripts/myos-c-userspace-lib.sh"
# shellcheck source=packages/image-libs-smoke/port.env
source "$HERE/port.env" # PORT_LIBC: the libc myos_write_cross_cc links

if myos_image_libs_smoke_is_current; then
  echo "image-libs-smoke up to date"
  exit 0
fi

"$ROOT/packages/image-libs/build.sh"
"$ROOT/toolchain/newlib/tool-wrappers.sh"
export PATH="$ROOT/target/newlib-bin:$PATH"

T="$ROOT/target"
WORK="$T/image-libs-smoke-build"
rm -rf "$WORK"
mkdir -p "$WORK"

for arch in x86_64 aarch64 riscv64; do
  echo "==> image-libs-smoke ($arch)"
  build="$WORK/$arch"
  out="$T/image-libs-mlibc-$arch"
  x11="$T/x11-libs-mlibc-$arch/lib/x11"
  mkdir -p "$build"
  myos_write_cross_cc "$arch" "$build/cc" -O2
  # shellcheck disable=SC2046
  "$build/cc" -Wall -Werror "$HERE/smoke.c" -o "$build/image-smoke" \
    -I"$x11/include" $(PKG_CONFIG_LIBDIR="$out/lib/pkgconfig" pkg-config --cflags --libs --static \
      libpng libjpeg libwebp pixman-1) \
    -L"$x11/lib" -lXcursor -lXrender -lXfixes -lXrandr -lXi -lXinerama -lXcomposite -lXdamage \
    -lXext -lX11 -lxcb -lXau -lm
  "${arch}-unknown-myos-strip" -o "$T/image-smoke-mlibc-${arch}-unknown-none" "$build/image-smoke"
done
rm -rf "$WORK"

myos_image_libs_smoke_version_hash >"$MYOS_IMAGE_LIBS_SMOKE_VERSION"
echo "image-libs-smoke -> target/image-smoke-mlibc-<arch>-unknown-none"
