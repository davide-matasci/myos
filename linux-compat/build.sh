#!/usr/bin/env bash
# Build the optional Linux compatibility layer's userspace pieces (x86_64):
#   target/linux-launcher-x86_64-unknown-none  the `linux` launcher (myos newlib)
#   target/linux-smoke-x86_64-linux-musl       a Linux static-PIE test (musl)
# musl is built from its release tarball with the host gcc. Only needed for
# `cargo build --features linux_compat`; see docs/linux-compat.md.
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# shellcheck source=scripts/myos-c-userspace-lib.sh
source "$ROOT/scripts/myos-c-userspace-lib.sh"

MUSL_VERSION=1.2.5
MUSL_SHA256=a9a118bbe84d8764da0ea0d28b3ab3fae8477fc7e4085d90102b8596fc7c75e4
MUSL_PREFIX="$ROOT/target/linux-compat/musl-$MUSL_VERSION"
LAUNCHER="$ROOT/target/linux-launcher-x86_64-unknown-none"
SMOKE="$ROOT/target/linux-smoke-x86_64-linux-musl"

# musl (static only) for the Linux-side test binaries.
if [[ ! -f "$MUSL_PREFIX/lib/libc.a" ]]; then
  work="$ROOT/target/linux-compat/build"
  rm -rf "$work"
  mkdir -p "$work"
  tarball="$work/musl-$MUSL_VERSION.tar.gz"
  curl -sSfL --retry 3 -o "$tarball" "https://musl.libc.org/releases/musl-$MUSL_VERSION.tar.gz"
  echo "$MUSL_SHA256  $tarball" | sha256sum -c -
  tar xzf "$tarball" -C "$work"
  (
    cd "$work/musl-$MUSL_VERSION"
    # The build host is x86_64 Linux: a native musl build is the target one.
    CC=gcc AR=ar RANLIB=ranlib ./configure --prefix="$MUSL_PREFIX" --disable-shared >/dev/null
    make -j"$(nproc)" >/dev/null 2>&1
    make install >/dev/null
  )
  rm -rf "$work"
fi

echo "==> linux-smoke (x86_64 Linux, musl static-PIE)"
gcc -static-pie -fPIE -O2 -nostdinc -nostdlib \
  -isystem "$MUSL_PREFIX/include" -isystem "$(gcc -print-file-name=include)" \
  -o "$SMOKE" \
  "$MUSL_PREFIX/lib/rcrt1.o" "$MUSL_PREFIX/lib/crti.o" \
  "$ROOT/linux-compat/tests/linux-smoke.c" \
  "$MUSL_PREFIX/lib/libc.a" "$(gcc -print-libgcc-file-name)" "$MUSL_PREFIX/lib/crtn.o"

echo "==> linux launcher (myos newlib)"
"$ROOT/toolchain/newlib/build.sh"
export PATH="$ROOT/target/newlib-bin:$PATH"
myos_ensure_llvm_bin
triple=x86_64-unknown-myos
prefix="$ROOT/target/newlib-x86_64"
obj="$ROOT/target/linux-launcher-x86_64.o"
"${triple}-cc" -ffreestanding -fPIC -O2 -isystem "$prefix/$triple/include" \
  -c "$ROOT/linux-compat/launcher.c" -o "$obj"
ld.lld -pie --no-dynamic-linker -o "$LAUNCHER" --entry=_start -z max-page-size=4096 \
  "$prefix/$triple/lib/crt0.o" "$obj" -L"$prefix/$triple/lib" \
  --start-group -lc -lgloss -lg --end-group
echo "linux-compat -> $LAUNCHER, $SMOKE"
