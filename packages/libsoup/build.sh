#!/usr/bin/env bash
# Cross-build the network libraries of WebKitGTK's stack (issue #616)
# against mlibc for the three arches: libxml2, libpsl (its public suffix
# list built in), nghttp2 (the library alone), libsoup 3 and
# glib-networking (its OpenSSL backend over LibreSSL, static), into one
# prefix per arch, target/libsoup-mlibc-<arch> (lib/ with the archives and
# their pkg-config files, include/). Over the mlibc builds of GLib,
# LibreSSL and SQLite. Then the package's smoke (smoke.c) over all of them.
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$HERE/../.." && pwd)"
# shellcheck source=scripts/myos-c-userspace-lib.sh
source "$ROOT/scripts/myos-c-userspace-lib.sh"
# shellcheck source=packages/libsoup/port.env
source "$HERE/port.env" # PORT_LIBC: the libc myos_write_cross_cc links
# shellcheck source=packages/libsoup/versions.env
source "$HERE/versions.env"
# shellcheck source=ports/libressl/versions.env
source "$ROOT/ports/libressl/versions.env"

if myos_libsoup_is_current; then
  echo "libsoup up to date"
  exit 0
fi

"$HERE/fetch.sh"
"$ROOT/toolchain/mlibc/build.sh"
"$ROOT/toolchain/libcxx/build.sh"
"$ROOT/packages/glib/build.sh"
"$ROOT/ports/libressl/build.sh"
"$ROOT/packages/sqlite/build.sh"
myos_ensure_llvm_bin
# The binutils: newlib's tool wrappers, <triple>-ar and -strip, over
# LLVM's (as packages/glib).
export PATH="$ROOT/target/newlib-bin:$PATH"
for tool in meson ninja pkg-config make; do
  if ! command -v "$tool" >/dev/null 2>&1; then
    echo "error: $tool not found (README.md)" >&2
    exit 1
  fi
done

T="$ROOT/target"

# pc DIR NAME VERSION PREFIX LIBS: a pkg-config file for a library built
# without one (LibreSSL's and SQLite's mlibc builds).
pc() {
  cat >"$1/$2.pc" <<EOF
Name: $2
Description: $2 ($PORT_LIBC build)
Version: $3
Cflags: -I$4/include
Libs: -L$4/lib $5
EOF
}

build_arch() {
  local arch="$1"
  local build="$T/libsoup-build-$arch"
  local out="$T/libsoup-mlibc-${arch}"
  local ssl="$T/libressl-mlibc-$arch" sqlite="$T/sqlite-mlibc-$arch"
  echo "==> libsoup ($arch, mlibc)"
  rm -rf "$build" "$out"
  mkdir -p "$out/lib/pkgconfig" "$out/include" "$build/pc" "$build/nghttp2"
  pc "$build/pc" libcrypto "$LIBRESSL_VERSION" "$ssl" -lcrypto
  pc "$build/pc" libssl "$LIBRESSL_VERSION" "$ssl" "-lssl -lcrypto"
  pc "$build/pc" openssl "$LIBRESSL_VERSION" "$ssl" "-lssl -lcrypto"
  pc "$build/pc" sqlite3 3 "$sqlite" -lsqlite3
  myos_mlibc_meson_cross "$arch" "$build" "$out/lib/pkgconfig" "$T/glib-mlibc-$arch/lib/pkgconfig" "$build/pc"

  # The XML parser, without iconv (mlibc has none; its own UTF-8, UTF-16
  # and Latin-1 encodings), zlib, ICU or its HTTP client.
  myos_mlibc_meson_build "$build" "$T/libxml2-src" "$out" -Diconv=disabled -Dzlib=disabled \
    -Dicu=disabled -Dhttp=disabled -Dmodules=disabled -Dpython=disabled -Dreadline=disabled \
    -Dhistory=disabled -Ddocs=disabled
  # The public suffix list built in (cookies' domains); no IDNA library.
  myos_mlibc_meson_build "$build" "$T/libpsl-src" "$out" -Druntime=no -Dbuiltin=true \
    -Dtests=false -Ddocs=false
  # HTTP/2: the library alone, with its autoconf build.
  local guess
  guess="$(sh "$T/nghttp2-src/config.guess")"
  (cd "$build/nghttp2" && CC="$build/cc" AR="${arch}-unknown-myos-ar" \
    RANLIB="${arch}-unknown-myos-ranlib" "$T/nghttp2-src/configure" \
    --host="${arch}-unknown-none" --build="$guess" --prefix="$out" --enable-lib-only \
    --enable-static --disable-shared >configure.log 2>&1) \
    || { tail -30 "$build/nghttp2/configure.log" >&2; exit 1; }
  make -C "$build/nghttp2" -j"$(nproc)" install >"$build/nghttp2/make.log" 2>&1 \
    || { grep -n "error:" "$build/nghttp2/make.log" | head -20 >&2; exit 1; }
  echo "nghttp2 -> $out"
  myos_mlibc_meson_build "$build" "$T/libsoup-src" "$out" -Dgssapi=disabled -Dntlm=disabled \
    -Dbrotli=disabled -Dzstd=disabled -Dtls_check=false -Dintrospection=disabled \
    -Dvapi=disabled -Ddocs=disabled -Ddoc_tests=false -Dtests=false -Dautobahn=disabled \
    -Dinstalled_tests=false -Dsysprof=disabled -Dpkcs11_tests=disabled
  # GIO's TLS over LibreSSL, as a static library the program registers
  # (glib-networking-static.myos.patch); no proxy modules.
  myos_mlibc_meson_build "$build" "$T/glib-networking-src" "$out" -Dgnutls=disabled \
    -Dopenssl=enabled -Dlibproxy=disabled -Dgnome_proxy=disabled -Denvironment_proxy=disabled \
    -Dtests=false -Dinstalled_tests=false

  # The smoke.
  local p="$out/lib/pkgconfig:$out/lib/gio/modules/pkgconfig:$T/glib-mlibc-$arch/lib/pkgconfig:$build/pc"
  local mods="libsoup-3.0 libxml-2.0 libpsl gioopenssl"
  # shellcheck disable=SC2046,SC2086
  "$build/cc" -Wall -Werror -c "$HERE/smoke.c" -o "$build/smoke.o" \
    $(PKG_CONFIG_LIBDIR="$p" pkg-config --cflags $mods)
  # shellcheck disable=SC2046,SC2086
  "$build/cc" "$build/smoke.o" -o "$T/libsoup-smoke-mlibc-${arch}-unknown-none" \
    $(PKG_CONFIG_LIBDIR="$p" pkg-config --libs --static $mods)
  "${arch}-unknown-myos-strip" "$T/libsoup-smoke-mlibc-${arch}-unknown-none"
  rm -rf "$build"
  echo "libsoup ($arch) -> $out"
}

for arch in ${MYOS_LIBSOUP_ARCHES:-x86_64 aarch64 riscv64}; do
  build_arch "$arch"
done

myos_libsoup_version_hash >"$MYOS_LIBSOUP_VERSION"
echo "libsoup build ok ($LIBSOUP_VERSION)"
