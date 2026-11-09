#!/usr/bin/env bash
# Link the text and drawing libraries' smoke (smoke.c) for the three arches
# against mlibc and packages/cairo-pango's archives (over GLib, the image
# libraries and the X libraries and font stack); C, linked as C++
# (HarfBuzz is). A package of its own, not cairo-pango's, so that a change
# to what it links and the libraries never compile against (the libc's C
# first) relinks it and rebuilds no library (docs/ports.md "Cache keys").
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$HERE/../.." && pwd)"
# shellcheck source=scripts/myos-c-userspace-lib.sh
source "$ROOT/scripts/myos-c-userspace-lib.sh"
# shellcheck source=packages/cairo-pango-smoke/port.env
source "$HERE/port.env" # PORT_LIBC: the libc myos_write_cross_cc links

if myos_cairo_pango_smoke_is_current; then
  echo "cairo-pango-smoke up to date"
  exit 0
fi

"$ROOT/packages/cairo-pango/build.sh"
"$ROOT/toolchain/newlib/tool-wrappers.sh"
export PATH="$ROOT/target/newlib-bin:$PATH"

T="$ROOT/target"
WORK="$T/cairo-pango-smoke-build"
rm -rf "$WORK"
mkdir -p "$WORK"

for arch in x86_64 aarch64 riscv64; do
  echo "==> cairo-pango-smoke ($arch)"
  build="$WORK/$arch"
  mkdir -p "$build"
  # The cross compilers and the X libraries' pkg-config files (x11-pc).
  myos_mlibc_meson_cross "$arch" "$build"
  pc="$T/cairo-pango-mlibc-$arch/lib/pkgconfig:$T/glib-mlibc-$arch/lib/pkgconfig"
  pc="$pc:$T/image-libs-mlibc-$arch/lib/pkgconfig:$build/x11-pc"
  mods="pangocairo pangoft2 gdk-pixbuf-2.0 cairo-xlib epoxy"
  # shellcheck disable=SC2046,SC2086
  "$build/cc" -Wall -Werror -c "$HERE/smoke.c" -o "$build/smoke.o" \
    $(PKG_CONFIG_LIBDIR="$pc" pkg-config --cflags $mods)
  # shellcheck disable=SC2046,SC2086
  "$build/c++" "$build/smoke.o" -o "$build/cairo-pango-smoke" \
    $(PKG_CONFIG_LIBDIR="$pc" pkg-config --libs --static $mods) -lm
  "${arch}-unknown-myos-strip" -o "$T/cairo-pango-smoke-mlibc-${arch}-unknown-none" "$build/cairo-pango-smoke"
done
rm -rf "$WORK"

myos_cairo_pango_smoke_version_hash >"$MYOS_CAIRO_PANGO_SMOKE_VERSION"
echo "cairo-pango-smoke -> target/cairo-pango-smoke-mlibc-<arch>-unknown-none"
