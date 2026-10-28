#!/usr/bin/env bash
# Cross-build the text and drawing libraries of GTK 3's stack (issue #616)
# against mlibc for the three arches: fribidi, harfbuzz (C++, over libc++),
# cairo (image, xlib and RENDER, PNG, FreeType and fontconfig; no xcb),
# pango (its cairo and FreeType backends), gdk-pixbuf (its loaders built
# in: there is no dlopen) and libepoxy, static, with meson, into one prefix
# per arch, target/cairo-pango-mlibc-<arch> (lib/ with the archives and
# their pkg-config files, include/). Over the mlibc builds of GLib, the
# image libraries, the X libraries and the font stack (their prefixes on
# pkg-config's path), and GLib's host glib-compile-resources. Then the
# package's smoke (smoke.c) over all of them.
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$HERE/../.." && pwd)"
# shellcheck source=scripts/myos-c-userspace-lib.sh
source "$ROOT/scripts/myos-c-userspace-lib.sh"
# shellcheck source=packages/cairo-pango/port.env
source "$HERE/port.env" # PORT_LIBC: the libc myos_write_cross_cc links
# shellcheck source=packages/cairo-pango/versions.env
source "$HERE/versions.env"

if myos_cairo_pango_is_current; then
  echo "cairo-pango up to date"
  exit 0
fi

"$HERE/fetch.sh"
"$ROOT/toolchain/mlibc/build.sh"
"$ROOT/toolchain/libcxx/build.sh"
"$ROOT/packages/glib/build.sh"
"$ROOT/packages/image-libs/build.sh"
"$ROOT/packages/x11-xft/build.sh"
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
SRC="$T/cairo-pango-src"

# meson_build NAME [OPTION...]: target/cairo-pango-src/NAME, cross
# (myos_mlibc_meson_build), into $out.
meson_build() {
  local name="$1"
  shift
  myos_mlibc_meson_build "$build" "$SRC/$name" "$out" "$@" || exit 1
}

build_arch() {
  local arch="$1"
  local build="$T/cairo-pango-build-$arch"
  local out="$T/cairo-pango-mlibc-${arch}"
  echo "==> cairo-pango ($arch, mlibc)"
  rm -rf "$build" "$out"
  mkdir -p "$out/lib/pkgconfig" "$out/include"
  local pc="$out/lib/pkgconfig:$T/glib-mlibc-$arch/lib/pkgconfig:$T/image-libs-mlibc-$arch/lib/pkgconfig:$build/x11-pc"
  myos_mlibc_meson_cross "$arch" "$build" "$out/lib/pkgconfig" "$T/glib-mlibc-$arch/lib/pkgconfig" \
    "$T/image-libs-mlibc-$arch/lib/pkgconfig"

  meson_build "fribidi-$FRIBIDI_VERSION" -Ddocs=false -Dbin=false -Dtests=false
  # Shaping over FreeType and GLib's Unicode tables; no hb-cairo (cairo is
  # built after it), no ICU, no tools.
  meson_build "harfbuzz-$HARFBUZZ_VERSION" -Dglib=enabled -Dgobject=disabled \
    -Dfreetype=enabled -Dcairo=disabled -Dicu=disabled -Dgraphite2=disabled \
    -Dchafa=disabled -Dpng=disabled -Dzlib=disabled -Draster=disabled -Dvector=disabled \
    -Dgpu=disabled -Dsubset=disabled -Dtests=disabled -Dintrospection=disabled \
    -Ddocs=disabled -Dutilities=disabled -Dbenchmark=disabled
  # The image surfaces, Xlib with RENDER (GTK 3's X backend), PNG,
  # FreeType and fontconfig; no xcb (x11-libs builds libxcb's core alone).
  meson_build "cairo-$CAIRO_VERSION" -Dxlib=enabled -Dxcb=disabled -Dxlib-xcb=disabled \
    -Dpng=enabled -Dfreetype=enabled -Dfontconfig=enabled -Dglib=enabled -Dzlib=enabled \
    -Dtee=disabled -Dtests=disabled -Dlzo=disabled -Dgtk2-utils=disabled \
    -Dspectre=disabled -Dsymbol-lookup=disabled -Dgtk_doc=false
  meson_build "pango-$PANGO_VERSION" -Dcairo=enabled -Dfreetype=enabled -Dfontconfig=enabled \
    -Dxft=disabled -Dlibthai=disabled -Dsysprof=disabled -Dintrospection=disabled \
    -Ddocumentation=false -Dman-pages=false -Dbuild-testsuite=false -Dbuild-examples=false
  # Every loader built in; PNG, JPEG and the others (GIF, BMP, ICO, ...),
  # no TIFF and no glycin (its loaders are separate processes). Formats
  # are told by the loaders' signatures, not GIO's sniffing (myos has no
  # shared-mime-info database).
  meson_build "gdk-pixbuf-$GDK_PIXBUF_VERSION" -Dpng=enabled -Djpeg=enabled -Dtiff=disabled \
    -Dgif=enabled -Dothers=enabled -Dglycin=disabled -Dbuiltin_loaders=all -Dgio_sniffing=false \
    -Dintrospection=disabled -Dman=false -Ddocumentation=false -Dtests=false \
    -Dinstalled_tests=false -Dthumbnailer=disabled
  # GTK 3 links libepoxy; no GL library is there to load at run time.
  # Its EGL dispatch is built over the Khronos headers, installed beside
  # it for WebKitGTK, which compiles against them and never calls EGL
  # (packages/webkitgtk). Their native window types are the opaque ones
  # (EGL_NO_PLATFORM_SPECIFIC_TYPES): myos is none of the platforms the
  # headers know, and WebKit's build says the same.
  cp -r "$SRC/egl-registry-$EGL_REGISTRY_COMMIT/EGL" "$SRC/egl-registry-$EGL_REGISTRY_COMMIT/KHR" \
    "$out/include/"
  meson_build "libepoxy-$LIBEPOXY_VERSION" -Dglx=yes -Degl=yes -Dx11=true -Dtests=false -Ddocs=false \
    -Dc_args="-D_GNU_SOURCE -DEGL_NO_PLATFORM_SPECIFIC_TYPES -I$out/include"

  # The smoke: pango and cairo, gdk-pixbuf, and cairo's Xlib surface; C,
  # linked as C++ (harfbuzz is).
  local mods="pangocairo pangoft2 gdk-pixbuf-2.0 cairo-xlib epoxy"
  # shellcheck disable=SC2046,SC2086
  "$build/cc" -Wall -Werror -c "$HERE/smoke.c" -o "$build/smoke.o" \
    $(PKG_CONFIG_LIBDIR="$pc" pkg-config --cflags $mods)
  # shellcheck disable=SC2046,SC2086
  "$build/c++" "$build/smoke.o" -o "$T/cairo-pango-smoke-mlibc-${arch}-unknown-none" \
    $(PKG_CONFIG_LIBDIR="$pc" pkg-config --libs --static $mods) -lm
  "${arch}-unknown-myos-strip" "$T/cairo-pango-smoke-mlibc-${arch}-unknown-none"
  rm -rf "$build"
  echo "cairo-pango ($arch) -> $out"
}

for arch in ${MYOS_CAIRO_PANGO_ARCHES:-x86_64 aarch64 riscv64}; do
  build_arch "$arch"
done

myos_cairo_pango_version_hash >"$MYOS_CAIRO_PANGO_VERSION"
echo "cairo-pango build ok"
