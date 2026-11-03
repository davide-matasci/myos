#!/usr/bin/env bash
# Stage packages/x11-libs' newlib build for tcc on myos, per arch, in
# target/x11-dev-<arch>/lib/x11: its headers, and its static libraries with
# libxcb and libXau merged into libX11.a. A static libX11 needs both, and a
# Makefile written for a shared one passes only -lX11 (dwmblocks). The
# separate libxcb.a and libXau.a stay too, for programs that name them.
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$HERE/../.." && pwd)"
# shellcheck source=scripts/myos-c-userspace-lib.sh
source "$ROOT/scripts/myos-c-userspace-lib.sh"

if myos_x11_dev_is_current; then
  echo "x11-dev up to date"
  exit 0
fi

"$ROOT/packages/x11-libs/build.sh"
AR="$(command -v llvm-ar 2>/dev/null || echo ar)"

for arch in x86_64 aarch64 riscv64; do
  src="$ROOT/target/x11-libs-$arch/lib/x11"
  out="$ROOT/target/x11-dev-$arch/lib/x11"
  rm -rf "$ROOT/target/x11-dev-$arch"
  mkdir -p "$out/lib"
  cp -a "$src/include" "$out/include"
  cp "$src"/lib/*.a "$out/lib/"
  rm "$out/lib/libX11.a"
  printf '%s\n' "CREATE $out/lib/libX11.a" "ADDLIB $src/lib/libX11.a" \
    "ADDLIB $src/lib/libxcb.a" "ADDLIB $src/lib/libXau.a" SAVE END \
    | "$AR" -M
done

myos_x11_dev_version_hash >"$MYOS_X11_DEV_VERSION"
echo "x11-dev -> target/x11-dev-<arch>"
