#!/usr/bin/env bash
# Link WebKitGTK's program, smoke.c, for the three arches against mlibc
# and packages/webkitgtk's archive (libwebkit2gtk-4.1.a and its pkg-config
# file, over GTK 3, libsoup and the rest), as surf is: its main renamed
# webkit_myos_program_main, which the archive's main (WebKit's
# MultiCall.cpp) runs, or WebKit's web or network process when the program
# runs itself again as one.
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$HERE/../.." && pwd)"
# shellcheck source=scripts/myos-c-userspace-lib.sh
source "$ROOT/scripts/myos-c-userspace-lib.sh"
# shellcheck source=packages/webkitgtk-smoke/port.env
source "$HERE/port.env" # PORT_LIBC: the libc myos_write_cross_cc links
# shellcheck source=ports/libressl/versions.env
source "$ROOT/ports/libressl/versions.env"

if myos_webkitgtk_smoke_is_current; then
  echo "webkitgtk-smoke up to date"
  exit 0
fi

"$ROOT/packages/webkitgtk/build.sh"
# The binutils: newlib's tool wrappers, <triple>-strip among them, written
# here: WebKit's build, current, exits before anything under it writes
# them, and a job whose newlib pull missed had none (PR #749).
"$ROOT/toolchain/newlib/tool-wrappers.sh"
export PATH="$ROOT/target/newlib-bin:$PATH"

T="$ROOT/target"
WORK="$T/webkitgtk-smoke-build"
rm -rf "$WORK"
mkdir -p "$WORK"

# pc DIR NAME VERSION PREFIX LIBS: a pkg-config file for a library built
# without one (LibreSSL's and SQLite's mlibc builds; as packages/surf).
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
  echo "==> webkitgtk-smoke ($arch)"
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
  # shellcheck disable=SC2046
  "$build/cc" -Os -Wall -Dmain=webkit_myos_program_main \
    $(PKG_CONFIG_LIBDIR="$pcdir" pkg-config --cflags webkit2gtk-4.1) \
    -c "$HERE/smoke.c" -o "$build/smoke.o"
  # shellcheck disable=SC2046
  "$build/c++" "$build/smoke.o" -o "$build/webkitgtk-smoke" \
    $(PKG_CONFIG_LIBDIR="$pcdir" pkg-config --static --libs webkit2gtk-4.1)
  "${arch}-unknown-myos-strip" -o "$T/webkitgtk-smoke-mlibc-${arch}-unknown-none" "$build/webkitgtk-smoke"
done
rm -rf "$WORK"

myos_webkitgtk_smoke_version_hash >"$MYOS_WEBKITGTK_SMOKE_VERSION"
echo "webkitgtk-smoke -> target/webkitgtk-smoke-mlibc-<arch>-unknown-none"
