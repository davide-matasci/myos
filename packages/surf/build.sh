#!/usr/bin/env bash
# Cross-build surf for the three arches against mlibc and packages/
# webkitgtk's archive (libwebkit2gtk-4.1.a and its pkg-config file, over
# GTK 3, libsoup and the rest), as that package's own program is built:
# surf's main renamed webkit_myos_program_main, which the archive's main
# (WebKit's MultiCall.cpp) runs, or WebKit's web or network process when
# the program runs itself again as one. Upstream surf with one myos patch
# (surf.myos.patch: no gcr, no web extension); config.h is the stock
# config.def.h. Then the boot test's helper, surf_check.c.
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$HERE/../.." && pwd)"
# shellcheck source=scripts/myos-c-userspace-lib.sh
source "$ROOT/scripts/myos-c-userspace-lib.sh"
# shellcheck source=packages/surf/port.env
source "$HERE/port.env" # PORT_LIBC: the libc myos_write_cross_cc links
# shellcheck source=packages/surf/versions.env
source "$HERE/versions.env"
# shellcheck source=ports/libressl/versions.env
source "$ROOT/ports/libressl/versions.env"

if myos_surf_is_current; then
  echo "surf up to date"
  exit 0
fi

"$HERE/fetch.sh"
"$ROOT/packages/webkitgtk/build.sh"
export PATH="$ROOT/target/newlib-bin:$PATH"

T="$ROOT/target"
WORK="$T/surf-build"
rm -rf "$WORK"
mkdir -p "$WORK"
cp -a "$T/surf-src" "$WORK/surf"
patch -d "$WORK/surf" -p1 --forward --batch -s <"$HERE/surf.myos.patch"
cp "$WORK/surf/config.def.h" "$WORK/surf/config.h"

# pc DIR NAME VERSION PREFIX LIBS: a pkg-config file for a library built
# without one (LibreSSL's and SQLite's mlibc builds; as packages/webkitgtk).
pc() {
  cat >"$1/$2.pc" <<EOP
Name: $2
Description: $2 ($PORT_LIBC build)
Version: $3
Cflags: -I$4/include
Libs: -L$4/lib $5
EOP
}

for arch in x86_64 aarch64 riscv64; do
  echo "==> surf ($arch)"
  build="$WORK/$arch"
  ssl="$T/libressl-mlibc-$arch"
  mkdir -p "$build/pc"
  myos_mlibc_meson_cross "$arch" "$build"
  pc "$build/pc" libcrypto "$LIBRESSL_VERSION" "$ssl" -lcrypto
  pc "$build/pc" libssl "$LIBRESSL_VERSION" "$ssl" "-lssl -lcrypto"
  pc "$build/pc" openssl "$LIBRESSL_VERSION" "$ssl" "-lssl -lcrypto"
  pc "$build/pc" sqlite3 3 "$T/sqlite-mlibc-$arch" -lsqlite3
  pcdir="$T/webkitgtk-mlibc-$arch/lib/pkgconfig"
  for m in zlib glib image-libs cairo-pango x11-xft gtk3 libsoup; do
    pcdir+=":$T/$m-mlibc-$arch/lib/pkgconfig"
  done
  pcdir+=":$T/libsoup-mlibc-$arch/lib/gio/modules/pkgconfig:$build/pc:$build/x11-pc"
  # surf's config.mk flags (its library directories unused: no web
  # extension), against WebKit, GTK and Xlib.
  # shellcheck disable=SC2046
  "$build/cc" -Os -D_DEFAULT_SOURCE -DVERSION=\"$SURF_VERSION\" -DLIBPREFIX=\"/lib\" \
    -DWEBEXTDIR=\"/lib/surf\" -Dmain=webkit_myos_program_main \
    $(PKG_CONFIG_LIBDIR="$pcdir" pkg-config --cflags webkit2gtk-4.1 x11) \
    -c "$WORK/surf/surf.c" -o "$build/surf.o"
  # shellcheck disable=SC2046
  "$build/c++" "$build/surf.o" -o "$build/surf" \
    $(PKG_CONFIG_LIBDIR="$pcdir" pkg-config --static --libs webkit2gtk-4.1 x11 gthread-2.0)
  "${arch}-unknown-myos-strip" -o "$T/surf-mlibc-${arch}-unknown-none" "$build/surf"
  # shellcheck disable=SC2046
  "$build/cc" -Wall -Wextra "$HERE/surf_check.c" -o "$T/surf-check-mlibc-${arch}-unknown-none" \
    $(PKG_CONFIG_LIBDIR="$build/x11-pc" pkg-config --cflags --static --libs x11)
done

myos_surf_version_hash >"$MYOS_SURF_VERSION"
echo "surf -> target/surf{,-check}-mlibc-<arch>-unknown-none"
