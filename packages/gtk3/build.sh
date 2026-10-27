#!/usr/bin/env bash
# Cross-build GTK 3 (its X11 backend) and ATK against mlibc for the three
# arches, static, with meson, into one prefix per arch,
# target/gtk3-mlibc-<arch> (lib/ with the archives and their pkg-config
# files, include/), what WebKitGTK and surf (issue #616) link. Over the
# mlibc builds of GLib, the image libraries, the text and drawing
# libraries (packages/cairo-pango) and the X libraries and font stack.
# Then the package's smoke (smoke.c) over all of them.
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$HERE/../.." && pwd)"
# shellcheck source=scripts/myos-c-userspace-lib.sh
source "$ROOT/scripts/myos-c-userspace-lib.sh"
# shellcheck source=packages/gtk3/port.env
source "$HERE/port.env" # PORT_LIBC: the libc myos_write_cross_cc links
# shellcheck source=packages/gtk3/versions.env
source "$HERE/versions.env"

if myos_gtk3_is_current; then
  echo "gtk3 up to date"
  exit 0
fi

"$HERE/fetch.sh"
"$ROOT/packages/cairo-pango/build.sh"
myos_ensure_llvm_bin
# The binutils: newlib's tool wrappers, <triple>-ar and -strip, over
# LLVM's (as packages/glib).
export PATH="$ROOT/target/newlib-bin:$PATH"
for tool in meson ninja pkg-config; do
  if ! command -v "$tool" >/dev/null 2>&1; then
    echo "error: $tool not found (README.md)" >&2
    exit 1
  fi
done

T="$ROOT/target"

build_arch() {
  local arch="$1"
  local build="$T/gtk3-build-$arch"
  local out="$T/gtk3-mlibc-${arch}"
  local libs="$T/glib-mlibc-$arch/lib/pkgconfig $T/image-libs-mlibc-$arch/lib/pkgconfig"
  libs="$libs $T/cairo-pango-mlibc-$arch/lib/pkgconfig"
  echo "==> gtk3 ($arch, mlibc)"
  rm -rf "$build" "$out"
  mkdir -p "$out/lib/pkgconfig" "$out/include"
  # shellcheck disable=SC2086
  myos_mlibc_meson_cross "$arch" "$build" "$out/lib/pkgconfig" $libs

  myos_mlibc_meson_build "$build" "$T/atk-src" "$out" -Ddocs=false -Dintrospection=false
  # The X11 backend alone, its input methods built in (myos has no
  # dlopen); no print backends (they are modules,
  # gtk-print-backends.myos.patch), no AT-SPI bridge (gtk-atk-bridge.myos.patch:
  # it needs D-Bus), no colord, no tracker.
  myos_mlibc_meson_build "$build" "$T/gtk-src" "$out" -Dx11_backend=true \
    -Dwayland_backend=false -Dbroadway_backend=false -Dxinerama=yes \
    -Dbuiltin_immodules=all -Dprint_backends= -Dcolord=no -Dcloudproviders=false \
    -Dprofiler=false -Dtracker3=false -Dintrospection=false -Ddemos=false \
    -Dexamples=false -Dtests=false -Dinstalled_tests=false -Dman=false -Dgtk_doc=false

  # The smoke: a GTK window on Xfbdev; C, linked as C++ (HarfBuzz is).
  local pc="$out/lib/pkgconfig" d
  for d in $libs "$build/x11-pc"; do
    pc="$pc:$d"
  done
  # shellcheck disable=SC2046
  "$build/cc" -Wall -Werror -c "$HERE/smoke.c" -o "$build/smoke.o" \
    $(PKG_CONFIG_LIBDIR="$pc" pkg-config --cflags gtk+-3.0)
  # shellcheck disable=SC2046
  "$build/c++" "$build/smoke.o" -o "$T/gtk3-smoke-mlibc-${arch}-unknown-none" \
    $(PKG_CONFIG_LIBDIR="$pc" pkg-config --libs --static gtk+-3.0) -lm
  "${arch}-unknown-myos-strip" "$T/gtk3-smoke-mlibc-${arch}-unknown-none"
  rm -rf "$build"
  echo "gtk3 ($arch) -> $out"
}

for arch in ${MYOS_GTK3_ARCHES:-x86_64 aarch64 riscv64}; do
  build_arch "$arch"
done

myos_gtk3_version_hash >"$MYOS_GTK3_VERSION"
echo "gtk3 build ok ($GTK_VERSION)"
