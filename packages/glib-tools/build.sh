#!/usr/bin/env bash
# Link GLib's programs for the three arches against mlibc and
# packages/glib's archives: gio (GIO's command, from the objects GLib's
# build leaves in lib/gio-tool) and the smoke (smoke.c, the package's
# test). A package of its own, not glib's, so that a change to what they
# link and GLib never compiles against (the libc's C first) relinks them
# and rebuilds no library (docs/ports.md "Cache keys").
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$HERE/../.." && pwd)"
# shellcheck source=scripts/myos-c-userspace-lib.sh
source "$ROOT/scripts/myos-c-userspace-lib.sh"
# shellcheck source=packages/glib-tools/port.env
source "$HERE/port.env" # PORT_LIBC: the libc myos_write_cross_cc links

if myos_glib_tools_is_current; then
  echo "glib-tools up to date"
  exit 0
fi

"$ROOT/packages/glib/build.sh"
# The binutils: newlib's tool wrappers, <triple>-strip among them (as
# packages/webkitgtk-smoke).
"$ROOT/toolchain/newlib/tool-wrappers.sh"
export PATH="$ROOT/target/newlib-bin:$PATH"

T="$ROOT/target"
WORK="$T/glib-tools-build"
rm -rf "$WORK"
mkdir -p "$WORK"

for arch in x86_64 aarch64 riscv64; do
  echo "==> glib-tools ($arch)"
  build="$WORK/$arch"
  glib="$T/glib-mlibc-$arch"
  mkdir -p "$build"
  myos_write_cross_cc "$arch" "$build/cc" -O2
  # shellcheck disable=SC2046
  "$build/cc" -Wall -Werror "$HERE/smoke.c" -o "$build/glib-smoke" \
    $(PKG_CONFIG_LIBDIR="$glib/lib/pkgconfig" pkg-config --cflags --libs --static gio-2.0)
  # shellcheck disable=SC2046
  "$build/cc" "$glib"/lib/gio-tool/*.o -o "$build/gio" \
    $(PKG_CONFIG_LIBDIR="$glib/lib/pkgconfig" pkg-config --libs --static gio-2.0)
  "${arch}-unknown-myos-strip" -o "$T/glib-smoke-mlibc-${arch}-unknown-none" "$build/glib-smoke"
  "${arch}-unknown-myos-strip" -o "$T/gio-mlibc-${arch}-unknown-none" "$build/gio"
done
rm -rf "$WORK"

myos_glib_tools_version_hash >"$MYOS_GLIB_TOOLS_VERSION"
echo "glib-tools -> target/{gio,glib-smoke}-mlibc-<arch>-unknown-none"
