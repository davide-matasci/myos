#!/usr/bin/env bash
# Cross-build newlib libc + myos libgloss for x86_64 and AArch64.
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$HERE/../.." && pwd)"
# shellcheck source=scripts/myos-c-userspace-lib.sh
source "$ROOT/scripts/myos-c-userspace-lib.sh"

if myos_newlib_is_current; then
  echo "newlib + libgloss up to date"
  exit 0
fi

"$ROOT/toolchain/newlib/fetch.sh"
"$ROOT/toolchain/newlib/patch.sh"
"$ROOT/toolchain/newlib/tool-wrappers.sh"

export PATH="$ROOT/target/newlib-bin:$PATH"

# HAVE_FCNTL: libc fcntl() must call _fcntl (libgloss), not return ENOSYS.
# Needed for oksh savefd/F_DUPFD (pipes). Also set via configure.host; this
# CFLAGS line is the reliable path.
TARGET_CFLAGS="-ffreestanding -fPIC -O2 -DHAVE_FCNTL"

build_one() {
  local arch="$1"
  local triple="${arch}-unknown-myos"
  local build="$ROOT/target/newlib-build-${arch}"
  local prefix="$ROOT/target/newlib-${arch}"

  echo "==> newlib libc ($triple) [HAVE_FCNTL]"
  rm -rf "$build"
  mkdir -p "$build"
  cd "$build"

  "$ROOT/target/newlib-src/configure" \
    --host=x86_64-pc-linux-gnu \
    --target="$triple" \
    --prefix="$prefix" \
    --disable-multilib \
    CC="${CC:-clang}" \
    CXX="${CXX:-clang++}" \
    CC_FOR_TARGET="${triple}-cc" \
    CXX_FOR_TARGET="${triple}-c++" \
    AS_FOR_TARGET="${triple}-as" \
    LD_FOR_TARGET="${triple}-ld" \
    AR_FOR_TARGET="${triple}-ar" \
    RANLIB_FOR_TARGET="${triple}-ranlib" \
    NM_FOR_TARGET="${triple}-nm" \
    CFLAGS_FOR_TARGET="$TARGET_CFLAGS" \
    CXXFLAGS_FOR_TARGET="$TARGET_CFLAGS"

  make -j"$(nproc)" all-target-newlib
  make install-target-newlib
  "$ROOT/toolchain/newlib/build-libgloss.sh" "$arch" "$prefix"
  echo "newlib + libgloss -> $prefix"
}

build_one x86_64
build_one aarch64
build_one riscv64

echo "$(myos_newlib_version_hash)" >"$MYOS_NEWLIB_VERSION"
