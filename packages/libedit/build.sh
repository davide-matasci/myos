#!/usr/bin/env bash
# Cross-build libedit as a static libedit.a against mlibc for the three
# arches, with its own configure (the target the compiler's triple: its
# config.sub does not know myos, and a cross build runs nothing it links),
# over the mlibc ncurses build (packages/ncurses) for the terminal
# capabilities: target/libedit-mlibc-<arch> (lib/libedit.a, include/
# histedit.h and editline/readline.h), what python's readline module links.
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$HERE/../.." && pwd)"
# shellcheck source=scripts/myos-c-userspace-lib.sh
source "$ROOT/scripts/myos-c-userspace-lib.sh"
# shellcheck source=packages/libedit/port.env
source "$HERE/port.env" # PORT_LIBC: the libc myos_write_cross_cc links
# shellcheck source=packages/libedit/versions.env
source "$HERE/versions.env"

if myos_libedit_is_current; then
  echo "libedit libs up to date"
  exit 0
fi

"$HERE/fetch.sh"
"$ROOT/toolchain/mlibc/build.sh"
"$ROOT/packages/ncurses/build.sh"
myos_ensure_llvm_bin
AR_BIN="$(command -v llvm-ar 2>/dev/null || echo ar)"
RANLIB_BIN="$(command -v llvm-ranlib 2>/dev/null || echo ranlib)"

SRC="$ROOT/target/libedit-src"

build_arch() {
  local arch="$1"
  local build="$ROOT/target/libedit-build-$arch"
  local out="$ROOT/target/libedit-mlibc-${arch}"
  local nc="$ROOT/target/ncurses-mlibc-${arch}"
  echo "==> libedit ($arch, mlibc)"
  rm -rf "$build" "$out"
  mkdir -p "$build"
  myos_write_cross_cc "$arch" "$build/cc" -O2
  (cd "$build" && CC="$build/cc" AR="$AR_BIN" RANLIB="$RANLIB_BIN" \
    CPPFLAGS="-I$nc/include" LDFLAGS="-L$nc/lib" \
    "$SRC/configure" --host="${arch}-unknown-none" --build="$("$SRC/config.guess")" \
      --prefix=/usr --enable-static --disable-shared --disable-examples >configure.log 2>&1) \
    || { tail -30 "$build/configure.log" >&2; exit 1; }
  make -C "$build/src" -j"$(nproc)" >"$build/make.log" 2>&1 \
    || { grep -n "error:" "$build/make.log" | head -20 >&2; tail -20 "$build/make.log" >&2; exit 1; }
  mkdir -p "$out/lib" "$out/include/editline"
  cp "$build/src/.libs/libedit.a" "$out/lib/libedit.a"
  # Its wcsdup.c is built whether or not the libc has one; mlibc's is in
  # the object with the other wide string calls libedit links, so the two
  # would clash: the libc's stays.
  if grep -q '^#define HAVE_WCSDUP 1' "$build/config.h"; then
    "$AR_BIN" d "$out/lib/libedit.a" wcsdup.o
  fi
  cp "$SRC/src/histedit.h" "$out/include/"
  cp "$SRC/src/editline/readline.h" "$out/include/editline/"
  rm -rf "$build"
  echo "libedit -> $out/lib/libedit.a"
}

for arch in ${MYOS_LIBEDIT_ARCHES:-x86_64 aarch64 riscv64}; do
  build_arch "$arch"
done

echo "$(myos_libedit_version_hash)" >"$MYOS_LIBEDIT_VERSION"
echo "libedit build ok ($LIBEDIT_VERSION)"
