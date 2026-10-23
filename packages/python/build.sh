#!/usr/bin/env bash
# Cross-build CPython against mlibc for the three arches (docs/mlibc.md,
# README.md here): target/python-<arch>/usr/{bin/python3.13,lib/python3.13},
# what the package carries (port.env). CPython's own build system does it
# (configure with the host triple host-myos.myos.patch teaches it, make),
# with a python of the same minor on the host for what a cross build cannot
# run (the frozen modules, the library's .pyc files): the host's, or this
# source built once for the host (target/python-host).
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$HERE/../.." && pwd)"
# shellcheck source=scripts/myos-c-userspace-lib.sh
source "$ROOT/scripts/myos-c-userspace-lib.sh"
# shellcheck source=packages/python/port.env
source "$HERE/port.env" # PORT_LIBC: the libc myos_write_cross_cc links
# shellcheck source=packages/python/versions.env
source "$HERE/versions.env"

if myos_python_is_current; then
  echo "python up to date"
  exit 0
fi

"$HERE/fetch.sh"
"$ROOT/ports/zlib/fetch.sh"
"$ROOT/toolchain/mlibc/build.sh"
myos_ensure_llvm_bin
# The binutils: newlib's build leaves <triple>-ar, -ranlib and -strip in
# target/newlib-bin, wrappers over LLVM's (the plain names, else the Rust
# sysroot's: the CI image has only the versioned ones), and GNU readelf
# reads every arch's ELF.
export PATH="$ROOT/target/newlib-bin:$PATH"
READELF_BIN="$(command -v llvm-readelf 2>/dev/null || echo readelf)"

JOBS="${MYOS_PYTHON_JOBS:-$(nproc 2>/dev/null || echo 4)}"
TARBALL="$ROOT/target/Python-${PYTHON_VERSION}.tar.xz"
SRC="$ROOT/target/python-src"
ZSRC="$ROOT/target/zlib-src"
MINOR="${PYTHON_VERSION%.*}"

rm -rf "$SRC"
mkdir -p "$SRC"
tar -xJf "$TARBALL" -C "$SRC" --strip-components=1
patch -d "$SRC" -p1 -s < "$HERE/host-myos.myos.patch"

# The build python: one of this minor on the host, else this source built
# for the host, kept across builds of the same version.
build_python() {
  local p out="$ROOT/target/python-host" build="$ROOT/target/python-host-build"
  for p in "python$MINOR" python3; do
    if command -v "$p" >/dev/null 2>&1 \
      && "$p" -c "import sys; sys.exit(sys.version_info[:2] != tuple(map(int, '$MINOR'.split('.'))))" 2>/dev/null; then
      command -v "$p"
      return
    fi
  done
  if [[ ! -x "$out/bin/python$MINOR" || "$(cat "$out/.version" 2>/dev/null)" != "$PYTHON_VERSION" ]]; then
    echo "==> python $PYTHON_VERSION for the host (it has no python $MINOR)" >&2
    rm -rf "$build" "$out"
    mkdir -p "$build"
    (cd "$build" && "$SRC/configure" --prefix="$out" --without-ensurepip --disable-test-modules >configure.log 2>&1 \
      && make -j"$JOBS" >make.log 2>&1 && make install >install.log 2>&1) \
      || { tail -20 "$build/make.log" >&2; exit 1; }
    echo "$PYTHON_VERSION" >"$out/.version"
  fi
  echo "$out/bin/python$MINOR"
}
BUILD_PYTHON="$(build_python)"
echo "build python: $BUILD_PYTHON"

build_arch() {
  local arch="$1"
  local build="$ROOT/target/python-build-$arch"
  local out="$ROOT/target/python-$arch"
  local lib="$out/usr/lib/python$MINOR"
  local f

  echo "==> python ($arch, mlibc)"
  rm -rf "$build" "$out"
  mkdir -p "$build/zlib"
  myos_write_cross_cc "$arch" "$build/cc" -O2

  # zlib against mlibc for the zlib module (the ports/zlib build is
  # newlib's: a libc's errno and stdio are its own).
  for f in adler32 compress crc32 deflate gzclose gzlib gzread gzwrite infback \
    inffast inflate inftrees trees uncompr zutil; do
    "$build/cc" -O2 -DHAVE_UNISTD_H -DHAVE_STDARG_H -D_LARGEFILE64_SOURCE=1 \
      -Wno-deprecated-non-prototype -c "$ZSRC/$f.c" -o "$build/zlib/$f.o"
  done
  "${arch}-unknown-myos-ar" rcs "$build/zlib/libz.a" "$build/zlib"/*.o
  cp "$ZSRC/zlib.h" "$ZSRC/zconf.h" "$build/zlib/"

  # configure, told what it cannot find out (config.site) and kept off the
  # host's libraries (PKG_CONFIG=false: with it, it took the host's libffi,
  # liblzma, readline and uuid for the target's). Every module static (no
  # dlopen in a static program); no mimalloc (it wants sys/syscall.h, a
  # Linux header); the bundled expat and libmpdec; /usr the prefix, built
  # into the interpreter: the app has the library at /usr/lib/python3.13
  # (the search upwards from the executable's directory, /bin/custom,
  # stops short of the root).
  (cd "$build" && CONFIG_SITE="$HERE/config.site" CC="$build/cc" CPP="$build/cc -E" \
    AR="${arch}-unknown-myos-ar" RANLIB="${arch}-unknown-myos-ranlib" READELF="$READELF_BIN" PKG_CONFIG=false \
    ZLIB_CFLAGS="-I$build/zlib" ZLIB_LIBS="-L$build/zlib -lz" MODULE_BUILDTYPE=static \
    "$SRC/configure" --host="${arch}-unknown-myos" --build="$("$SRC/config.guess")" \
      --with-build-python="$BUILD_PYTHON" --prefix=/usr --disable-shared \
      --disable-test-modules --without-ensurepip --without-readline --without-mimalloc \
      --with-system-expat=no --with-system-libmpdec=no >configure.log 2>&1) \
    || { tail -30 "$build/configure.log" >&2; exit 1; }
  make -C "$build" -j"$JOBS" >"$build/make.log" 2>&1 \
    || { grep -n "error:" "$build/make.log" | head -20 >&2; tail -20 "$build/make.log" >&2; exit 1; }
  make -C "$build" install DESTDIR="$out" >"$build/install.log" 2>&1 \
    || { tail -20 "$build/install.log" >&2; exit 1; }

  # What the app carries: the interpreter and the library, without the
  # test suite, IDLE, Tk, turtle and ensurepip (no pip without ssl), the
  # .pyc files (half the files and bytes of the library: the interpreter
  # writes them into the app's directory on the first import), the
  # headers, libpython, the manuals and the scripts beside the
  # interpreter.
  rm -rf "$lib"/test "$lib"/idlelib "$lib"/tkinter "$lib"/turtledemo "$lib"/turtle.py \
    "$lib"/ensurepip "$lib"/config-"$MINOR"* "$out/usr/include" "$out/usr/share" \
    "$out/usr/lib/pkgconfig" "$out/usr/lib"/libpython*
  find "$lib" -name __pycache__ -type d -prune -exec rm -rf {} +
  find "$out/usr/bin" -mindepth 1 ! -name "python$MINOR" -delete
  "${arch}-unknown-myos-strip" -s "$out/usr/bin/python$MINOR"
  echo "python -> $out/usr/bin/python$MINOR ($(du -h "$out/usr/bin/python$MINOR" | cut -f1)), library $(du -sh "$lib" | cut -f1)"
}

# Default: all arches (CI). MYOS_PYTHON_ARCHES=x86_64 for a faster local build.
ARCHES="${MYOS_PYTHON_ARCHES:-x86_64 aarch64 riscv64}"
# shellcheck disable=SC2086
for arch in $ARCHES; do
  build_arch "$arch"
done

if myos_python_outputs_present; then
  myos_python_version_hash >"$MYOS_PYTHON_VERSION"
else
  echo "warning: not all arch outputs present; stamp not written (partial build)" >&2
fi
