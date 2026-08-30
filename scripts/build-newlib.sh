#!/usr/bin/env bash
# Cross-build newlib libc + myos libgloss for x86_64 and AArch64.
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# shellcheck source=scripts/myos-c-userspace-lib.sh
source "$ROOT/scripts/myos-c-userspace-lib.sh"

if myos_newlib_is_current; then
  echo "newlib + libgloss up to date"
  exit 0
fi

"$ROOT/scripts/fetch-newlib.sh"
"$ROOT/scripts/patch-newlib-myos.sh"
"$ROOT/scripts/newlib-tool-wrappers.sh"

export PATH="$ROOT/target/newlib-bin:$PATH"

build_one() {
  local arch="$1"
  local triple="${arch}-unknown-myos"
  local build="$ROOT/target/newlib-build-${arch}"
  local prefix="$ROOT/target/newlib-${arch}"

  echo "==> newlib libc ($triple)"
  rm -rf "$build"
  mkdir -p "$build"
  cd "$build"

  "$ROOT/target/newlib-src/configure" \
    --host=x86_64-pc-linux-gnu \
    --target="$triple" \
    --prefix="$prefix" \
    --disable-multilib \
    CC=gcc \
    CXX=g++ \
    CC_FOR_TARGET="${triple}-cc" \
    CXX_FOR_TARGET="${triple}-c++" \
    AS_FOR_TARGET="${triple}-as" \
    LD_FOR_TARGET="${triple}-ld" \
    AR_FOR_TARGET="${triple}-ar" \
    RANLIB_FOR_TARGET="${triple}-ranlib" \
    NM_FOR_TARGET="${triple}-nm" \
    CFLAGS_FOR_TARGET="-ffreestanding -fPIC -O2" \
    CXXFLAGS_FOR_TARGET="-ffreestanding -fPIC -O2"

  make -j"$(nproc)" all-target-newlib
  make install-target-newlib
  "$ROOT/scripts/build-libgloss-myos.sh" "$arch" "$prefix"
  echo "newlib + libgloss -> $prefix"
}

build_one x86_64
build_one aarch64

echo "$(myos_newlib_version_hash)" >"$MYOS_NEWLIB_VERSION"
