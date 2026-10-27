#!/usr/bin/env bash
# Cross-build GLib (GLib, GObject, GIO, GModule, GThread) as static
# libraries against mlibc for the three arches, with what it links that
# mlibc lacks: libffi, GNU libiconv and proxy-libintl (gettext returning
# its message), and the mlibc builds of zlib (ports/zlib) and PCRE2
# (packages/pcre2). One prefix per arch, target/glib-mlibc-<arch> (lib/
# with the archives and their pkg-config files, include/), what a program
# over GLib (WebKitGTK's stack, issue #616) compiles and links against.
# And GLib's tools that run on the build host, built natively from the
# same pins into target/glib-host (bin/glib-compile-resources and the
# others): the libraries over GLib compile their resources, schemas and
# D-Bus interfaces with them.
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$HERE/../.." && pwd)"
# shellcheck source=scripts/myos-c-userspace-lib.sh
source "$ROOT/scripts/myos-c-userspace-lib.sh"
# shellcheck source=packages/glib/port.env
source "$HERE/port.env" # PORT_LIBC: the libc myos_write_cross_cc links
# shellcheck source=packages/glib/versions.env
source "$HERE/versions.env"

if myos_glib_is_current; then
  echo "glib libs up to date"
  exit 0
fi

"$HERE/fetch.sh"
"$ROOT/toolchain/mlibc/build.sh"
"$ROOT/ports/zlib/build.sh"
"$ROOT/packages/pcre2/build.sh"
myos_ensure_llvm_bin
# The binutils: newlib's tool wrappers, <triple>-ar, -ranlib and -strip
# (zlib's and PCRE2's builds make them), over LLVM's: the host's strip
# does not know aarch64 and riscv64, and the CI image has LLVM's under
# versioned names only.
export PATH="$ROOT/target/newlib-bin:$PATH"
for tool in meson ninja pkg-config; do
  if ! command -v "$tool" >/dev/null 2>&1; then
    echo "error: $tool not found (README.md)" >&2
    exit 1
  fi
done

T="$ROOT/target"

# configure_make NAME SRC DIR [ARG...]: an autoconf build of SRC in DIR
# with the arch's cross cc, installed into $out.
configure_make() {
  local name="$1" src="$2" dir="$3" guess
  shift 3
  guess="$src/config.guess"
  [[ -f "$guess" ]] || guess="$src/build-aux/config.guess"
  (cd "$dir" && CC="${cc[0]}" AR="$AR_BIN" RANLIB="$RANLIB_BIN" \
    "$src/configure" --host="${arch}-unknown-none" --build="$(sh "$guess")" \
      --prefix="$out" --enable-static --disable-shared "$@" >configure.log 2>&1) \
    || { tail -30 "$dir/configure.log" >&2; exit 1; }
  make -C "$dir" -j"$(nproc)" >"$dir/make.log" 2>&1 \
    || { grep -n "error:" "$dir/make.log" | head -20 >&2; tail -20 "$dir/make.log" >&2; exit 1; }
  make -C "$dir" install >"$dir/install.log" 2>&1 || { tail -20 "$dir/install.log" >&2; exit 1; }
  echo "$name ($arch) -> $out"
}

# pc NAME VERSION LIBS: a pkg-config file for a library of $out.
pc() {
  cat >"$out/lib/pkgconfig/$1.pc" <<EOF
prefix=$out
libdir=\${prefix}/lib
includedir=\${prefix}/include

Name: $1
Description: $1 (myos, mlibc)
Version: $2
Libs: -L\${libdir} $3
Cflags: -I\${includedir}
EOF
}

build_arch() {
  local arch="$1"
  local build="$T/glib-build-$arch"
  local out="$T/glib-mlibc-${arch}"
  local AR_BIN="${arch}-unknown-myos-ar" RANLIB_BIN="${arch}-unknown-myos-ranlib"
  local f
  echo "==> glib ($arch, mlibc)"
  rm -rf "$build" "$out"
  mkdir -p "$build"/{libffi,libiconv,intl} "$out/lib/pkgconfig" "$out/include"
  myos_write_cross_cc "$arch" "$build/cc" -O2
  local cc=("$build/cc")

  # zlib (ports/zlib's mlibc build).
  cp "$T/zlib-mlibc-$arch/lib/libz.a" "$out/lib/"
  cp "$T/zlib-mlibc-$arch/include/zlib.h" "$T/zlib-mlibc-$arch/include/zconf.h" "$out/include/"
  pc zlib "$(. "$ROOT/ports/zlib/versions.env"; echo "$ZLIB_VERSION")" -lz

  # PCRE2 (packages/pcre2's mlibc build).
  cp "$T/pcre2-mlibc-$arch/lib/libpcre2-8.a" "$out/lib/"
  cp "$T/pcre2-mlibc-$arch/include/pcre2.h" "$out/include/"
  pc libpcre2-8 "$(. "$ROOT/packages/pcre2/versions.env"; echo "$PCRE2_VERSION")" -lpcre2-8
  sed -i 's|^Cflags:.*|& -DPCRE2_STATIC|' "$out/lib/pkgconfig/libpcre2-8.pc"

  # proxy-libintl, without its dlopen of a real libintl (STUB_ONLY).
  "${cc[@]}" -DSTUB_ONLY -DG_INTL_STATIC_COMPILATION -c "$T/proxy-libintl-src/libintl.c" \
    -o "$build/intl/libintl.o"
  "$AR_BIN" rcs "$out/lib/libintl.a" "$build/intl/libintl.o"
  cp "$T/proxy-libintl-src/libintl.h" "$out/include/"

  # libffi: GObject's generic marshaller calls through it. Closures map
  # their trampolines writable and executable.
  configure_make libffi "$T/libffi-src" "$build/libffi" --disable-docs \
    --disable-multi-os-directory --disable-exec-static-tramp

  # GNU libiconv.
  configure_make libiconv "$T/libiconv-src" "$build/libiconv" --disable-nls

  # GLib with meson, cross, with the cross cc (clang for the bare target
  # does not link for every arch; that cc runs ld.lld as myos_mlibc_link
  # does). needs_exe_wrapper: meson runs nothing it builds; GLib's run
  # checks take their defaults (its own printf, gnulib's, for the C99 and
  # positional ones).
  local cpu="$arch"
  case "$arch" in aarch64) cpu=armv8-a ;; riscv64) cpu=rv64gc ;; esac
  cat >"$build/cross.txt" <<EOC
[binaries]
c = '${cc[0]}'
ar = '$AR_BIN'
strip = '${arch}-unknown-myos-strip'
pkg-config = 'pkg-config'

[built-in options]
c_args = ['-I$out/include', '-D_GNU_SOURCE', '-D__myos__=1']
c_link_args = ['-L$out/lib']

[properties]
needs_exe_wrapper = true
pkg_config_libdir = '$out/lib/pkgconfig'

[host_machine]
system = 'myos'
cpu_family = '$arch'
cpu = '$cpu'
endian = 'little'
EOC
  meson setup "$build/glib" "$T/glib-src" --cross-file "$build/cross.txt" \
    --prefix "$out" --libdir lib --includedir include --buildtype release \
    -Ddefault_library=static -Dtests=false -Dinstalled_tests=false \
    -Dintrospection=disabled -Dnls=disabled -Dlibmount=disabled -Dselinux=disabled \
    -Dxattr=false -Dsysprof=disabled -Ddtrace=disabled -Dsystemtap=disabled \
    -Dman-pages=disabled -Ddocumentation=false -Dlibelf=disabled \
    -Dglib_debug=disabled -Dbsymbolic_functions=false \
    --wrap-mode=nofallback >"$build/setup.log" 2>&1 || { tail -40 "$build/setup.log" >&2; exit 1; }
  ninja -C "$build/glib" >"$build/ninja.log" 2>&1 \
    || { grep -n "error" "$build/ninja.log" | head -20 >&2; tail -30 "$build/ninja.log" >&2; exit 1; }
  ninja -C "$build/glib" install >"$build/install.log" 2>&1 || { tail -20 "$build/install.log" >&2; exit 1; }
  rm -f "$out"/lib/*.la

  # The app: the smoke (smoke.c, the package's test) and GIO's gio.
  # shellcheck disable=SC2046
  "${cc[@]}" -Wall -Werror "$HERE/smoke.c" -o "$T/glib-smoke-mlibc-${arch}-unknown-none" \
    $(PKG_CONFIG_LIBDIR="$out/lib/pkgconfig" pkg-config --cflags --libs --static gio-2.0)
  cp "$out/bin/gio" "$T/gio-mlibc-${arch}-unknown-none"
  "${arch}-unknown-myos-strip" "$T/glib-smoke-mlibc-${arch}-unknown-none" "$T/gio-mlibc-${arch}-unknown-none"
  rm -rf "$build"
  echo "glib ($arch) -> $out"
}

# GLib for the build host, with the host's compiler, over host builds of
# libffi, PCRE2 and zlib from the same pins: its glib-compile-resources
# and glib-compile-schemas into target/glib-host/bin (the cross build's
# run on myos only), with its Python generators, glib-mkenums,
# glib-genmarshal and gdbus-codegen, configured for the host.
build_host() {
  local build="$T/glib-host-build" out="$T/glib-host"
  echo "==> glib (build host)"
  rm -rf "$build" "$out"
  mkdir -p "$build"/{libffi,pcre2,zlib} "$out/bin"
  (cd "$build/libffi" && "$T/libffi-src/configure" --prefix="$build/prefix" --disable-shared \
      --disable-docs --disable-multi-os-directory >configure.log 2>&1 && make -j"$(nproc)" install \
      >make.log 2>&1) || { tail -20 "$build/libffi/make.log" "$build/libffi/configure.log" >&2; exit 1; }
  (cd "$build/pcre2" && "$T/pcre2-src/configure" --prefix="$build/prefix" --disable-shared \
      >configure.log 2>&1 && make -j"$(nproc)" install >make.log 2>&1) \
    || { tail -20 "$build/pcre2/make.log" "$build/pcre2/configure.log" >&2; exit 1; }
  cp -a "$T/zlib-src/." "$build/zlib/"
  (cd "$build/zlib" && ./configure --prefix="$build/prefix" --static >configure.log 2>&1 \
      && make -j"$(nproc)" install >make.log 2>&1) || { tail -20 "$build/zlib/make.log" >&2; exit 1; }
  PKG_CONFIG_PATH="$build/prefix/lib/pkgconfig" meson setup "$build/glib" "$T/glib-src" \
    --buildtype release -Ddefault_library=static -Dtests=false -Dinstalled_tests=false \
    -Dintrospection=disabled -Dnls=disabled -Dlibmount=disabled -Dselinux=disabled \
    -Dxattr=false -Dsysprof=disabled -Ddtrace=disabled -Dsystemtap=disabled \
    -Dman-pages=disabled -Ddocumentation=false -Dlibelf=disabled -Dglib_debug=disabled \
    --wrap-mode=nofallback >"$build/setup.log" 2>&1 || { tail -40 "$build/setup.log" >&2; exit 1; }
  ninja -C "$build/glib" gio/glib-compile-resources gio/glib-compile-schemas >"$build/ninja.log" 2>&1 \
    || { grep -n "error" "$build/ninja.log" | head -20 >&2; exit 1; }
  cp "$build/glib/gio/glib-compile-resources" "$build/glib/gio/glib-compile-schemas" \
    "$build/glib/gobject/glib-mkenums" "$build/glib/gobject/glib-genmarshal" \
    "$build/glib/gio/gdbus-2.0/codegen/gdbus-codegen" "$out/bin/"
  # gdbus-codegen finds its modules in ../share/glib-2.0/codegen.
  mkdir -p "$out/share/glib-2.0/codegen"
  cp "$T"/glib-src/gio/gdbus-2.0/codegen/*.py "$build/glib/gio/gdbus-2.0/codegen/config.py" \
    "$out/share/glib-2.0/codegen/"
  rm -rf "$build"
  echo "glib (build host) -> $out/bin"
}

build_host
for arch in ${MYOS_GLIB_ARCHES:-x86_64 aarch64 riscv64}; do
  build_arch "$arch"
done

echo "$(myos_glib_version_hash)" >"$MYOS_GLIB_VERSION"
echo "glib build ok ($GLIB_VERSION)"
