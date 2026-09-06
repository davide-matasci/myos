#!/usr/bin/env bash
# Cross-build static libncurses.a (termcap + tinfo + base) for myos arches.
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$HERE/../.." && pwd)"
# shellcheck source=scripts/myos-c-userspace-lib.sh
source "$ROOT/scripts/myos-c-userspace-lib.sh"

if myos_ncurses_is_current; then
  echo "ncurses libs up to date"
  exit 0
fi

"$ROOT/ports/ncurses/prepare.sh"
"$ROOT/toolchain/newlib/build.sh"
export PATH="$ROOT/target/newlib-bin:$PATH"

WORK="$ROOT/target/ncurses-myos-build"

build_arch() {
  local arch="$1"
  local triple="${arch}-unknown-myos"
  local prefix="$ROOT/target/newlib-${arch}"
  local inc="$prefix/${triple}/include"
  local cc="${triple}-cc"
  local out="$ROOT/target/ncurses-${arch}"
  local clanginc
  clanginc="$("$cc" -print-resource-dir)/include"

  # Ensure central termios.h (baud rates, etc.) is in the sysroot.
  cp "$ROOT/toolchain/newlib/libgloss/myos/termios.h" "$inc/termios.h"

  echo "==> ncurses ($triple)"
  # Wipe only cross objects/archive so host helpers (make_hash, make_keys) stay.
  rm -f "$WORK"/objects/*.o "$WORK/lib/libncurses.a"

  # Build only the archive — `make libs` also builds host report_offsets.
  # Keep BUILD_* free of -nostdinc / myos -isystem (those break host helpers).
  make -C "$WORK/ncurses" \
    CC="$cc" \
    BUILD_CC=gcc \
    BUILD_CFLAGS="-O2" \
    BUILD_CPPFLAGS="-DHAVE_CONFIG_H -I$WORK/ncurses -I$ROOT/target/ncurses-src/ncurses -I$WORK/include -I$ROOT/target/ncurses-src/include" \
    CFLAGS="-ffreestanding -fPIC -O2 -std=gnu99" \
    CPPFLAGS="-DHAVE_CONFIG_H -I$WORK/ncurses -I$ROOT/target/ncurses-src/ncurses -I$WORK/include -I$ROOT/target/ncurses-src/include -nostdinc -isystem $clanginc -isystem $inc -I$ROOT/toolchain/newlib/libgloss/myos -D_DEFAULT_SOURCE -D_GNU_SOURCE" \
    ../lib/libncurses.a

  rm -rf "$out"
  mkdir -p "$out/lib" "$out/include"
  cp "$WORK/lib/libncurses.a" "$out/lib/libncurses.a"
  # Also expose as libtinfo for consumers that expect a split tinfo.
  cp "$WORK/lib/libncurses.a" "$out/lib/libtinfo.a"
  for h in curses.h term.h termcap.h unctrl.h ncurses_dll.h; do
    if [[ -f "$WORK/include/$h" ]]; then
      cp "$WORK/include/$h" "$out/include/$h"
    fi
  done
  cp "$out/lib/libncurses.a" "$ROOT/target/libncurses-${triple}.a"
  echo "ncurses -> $out/lib/libncurses.a"
}

for arch in x86_64 aarch64 riscv64; do
  build_arch "$arch"
done

echo "$(myos_ncurses_version_hash)" >"$MYOS_NCURSES_VERSION"
