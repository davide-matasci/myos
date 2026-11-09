#!/usr/bin/env bash
# Link GTK 3's smoke (smoke.c) for the three arches against mlibc and
# packages/gtk3's archives (over the text, image and X libraries and
# GLib); C, linked as C++ (HarfBuzz is). A package of its own, not gtk3's,
# so that a change to what it links and GTK never compiles against (the
# libc's C first) relinks it and rebuilds no library (docs/ports.md "Cache
# keys").
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$HERE/../.." && pwd)"
# shellcheck source=scripts/myos-c-userspace-lib.sh
source "$ROOT/scripts/myos-c-userspace-lib.sh"
# shellcheck source=packages/gtk3-smoke/port.env
source "$HERE/port.env" # PORT_LIBC: the libc myos_write_cross_cc links

if myos_gtk3_smoke_is_current; then
  echo "gtk3-smoke up to date"
  exit 0
fi

"$ROOT/packages/gtk3/build.sh"
"$ROOT/toolchain/newlib/tool-wrappers.sh"
export PATH="$ROOT/target/newlib-bin:$PATH"

T="$ROOT/target"
WORK="$T/gtk3-smoke-build"
rm -rf "$WORK"
mkdir -p "$WORK"

for arch in x86_64 aarch64 riscv64; do
  echo "==> gtk3-smoke ($arch)"
  build="$WORK/$arch"
  mkdir -p "$build"
  # The cross compilers and the X libraries' pkg-config files (x11-pc).
  myos_mlibc_meson_cross "$arch" "$build"
  pc="$T/gtk3-mlibc-$arch/lib/pkgconfig:$T/glib-mlibc-$arch/lib/pkgconfig"
  pc="$pc:$T/image-libs-mlibc-$arch/lib/pkgconfig:$T/cairo-pango-mlibc-$arch/lib/pkgconfig:$build/x11-pc"
  # shellcheck disable=SC2046
  "$build/cc" -Wall -Werror -c "$HERE/smoke.c" -o "$build/smoke.o" \
    $(PKG_CONFIG_LIBDIR="$pc" pkg-config --cflags gtk+-3.0)
  # shellcheck disable=SC2046
  "$build/c++" "$build/smoke.o" -o "$build/gtk3-smoke" \
    $(PKG_CONFIG_LIBDIR="$pc" pkg-config --libs --static gtk+-3.0) -lm
  "${arch}-unknown-myos-strip" -o "$T/gtk3-smoke-mlibc-${arch}-unknown-none" "$build/gtk3-smoke"
done
rm -rf "$WORK"

myos_gtk3_smoke_version_hash >"$MYOS_GTK3_SMOKE_VERSION"
echo "gtk3-smoke -> target/gtk3-smoke-mlibc-<arch>-unknown-none"
