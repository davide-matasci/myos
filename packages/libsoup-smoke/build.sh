#!/usr/bin/env bash
# Link the network libraries' smoke (smoke.c) for the three arches against
# mlibc and packages/libsoup's archives: libsoup, libxml2 and libpsl, and
# GIO's TLS backend over LibreSSL, which the program registers. A package
# of its own, not libsoup's, so that a change to what it links and the
# libraries never compile against (the libc's C, the TLS backend, LibreSSL)
# relinks it and rebuilds no library (docs/ports.md "Cache keys").
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$HERE/../.." && pwd)"
# shellcheck source=scripts/myos-c-userspace-lib.sh
source "$ROOT/scripts/myos-c-userspace-lib.sh"
# shellcheck source=packages/libsoup-smoke/port.env
source "$HERE/port.env" # PORT_LIBC: the libc myos_write_cross_cc links
# shellcheck source=ports/libressl/versions.env
source "$ROOT/ports/libressl/versions.env"

if myos_libsoup_smoke_is_current; then
  echo "libsoup-smoke up to date"
  exit 0
fi

"$ROOT/packages/libsoup/build.sh"
"$ROOT/toolchain/newlib/tool-wrappers.sh"
export PATH="$ROOT/target/newlib-bin:$PATH"

T="$ROOT/target"
WORK="$T/libsoup-smoke-build"
rm -rf "$WORK"
mkdir -p "$WORK"

# pc DIR NAME VERSION PREFIX LIBS: a pkg-config file for a library built
# without one (LibreSSL's and SQLite's mlibc builds; as packages/libsoup).
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
  echo "==> libsoup-smoke ($arch)"
  build="$WORK/$arch"
  out="$T/libsoup-mlibc-$arch"
  ssl="$T/libressl-mlibc-$arch"
  mkdir -p "$build/pc"
  myos_write_cross_cc "$arch" "$build/cc" -O2
  pc "$build/pc" libcrypto "$LIBRESSL_VERSION" "$ssl" -lcrypto
  pc "$build/pc" libssl "$LIBRESSL_VERSION" "$ssl" "-lssl -lcrypto"
  pc "$build/pc" openssl "$LIBRESSL_VERSION" "$ssl" "-lssl -lcrypto"
  pc "$build/pc" sqlite3 3 "$T/sqlite-mlibc-$arch" -lsqlite3
  p="$out/lib/pkgconfig:$out/lib/gio/modules/pkgconfig:$T/glib-mlibc-$arch/lib/pkgconfig:$build/pc"
  mods="libsoup-3.0 libxml-2.0 libpsl gioopenssl"
  # shellcheck disable=SC2046,SC2086
  "$build/cc" -Wall -Werror -c "$HERE/smoke.c" -o "$build/smoke.o" \
    $(PKG_CONFIG_LIBDIR="$p" pkg-config --cflags $mods)
  # shellcheck disable=SC2046,SC2086
  "$build/cc" "$build/smoke.o" -o "$build/libsoup-smoke" \
    $(PKG_CONFIG_LIBDIR="$p" pkg-config --libs --static $mods)
  "${arch}-unknown-myos-strip" -o "$T/libsoup-smoke-mlibc-${arch}-unknown-none" "$build/libsoup-smoke"
done
rm -rf "$WORK"

myos_libsoup_smoke_version_hash >"$MYOS_LIBSOUP_SMOKE_VERSION"
echo "libsoup-smoke -> target/libsoup-smoke-mlibc-<arch>-unknown-none"
