#!/usr/bin/env bash
# Cross-build ncurses' `clear` program (clears the terminal) for the myos
# arches, linked against the static libncurses.a the ncurses package builds.
# No own source pin: the three progs/*.c files come from the ncurses source
# tree (ports/packages share one checkout under target/ncurses-src).
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$HERE/../.." && pwd)"
# shellcheck source=scripts/myos-c-userspace-lib.sh
source "$ROOT/scripts/myos-c-userspace-lib.sh"

if myos_clear_is_current; then
  echo "clear ELFs up to date"
  exit 0
fi

# ncurses prepares the source tree (target/ncurses-src), generates its headers
# and builds libncurses.a for the three arches; clear reuses all of it.
"$(myos_port_dir ncurses)/build.sh"
"$ROOT/toolchain/newlib/build.sh"
export PATH="$ROOT/target/newlib-bin:$PATH"

SRC="$ROOT/target/ncurses-src"
WORK="$ROOT/target/ncurses-myos-build"

# clear.c + its two helpers; the rest (terminfo lookup, tputs) is in libncurses.
CLEAR_SRCS=(clear clear_cmd tty_settings)

build_arch() {
  local arch="$1"
  local triple="${arch}-unknown-myos"
  local prefix="$ROOT/target/newlib-${arch}"
  local inc="$prefix/${triple}/include"
  local lib="$prefix/${triple}/lib"
  local nclib="$ROOT/target/ncurses-${arch}/lib"
  local cc="${triple}-cc"
  local ld="${triple}-ld"
  local objdir="$ROOT/target/clear-obj-${arch}"
  local out="$ROOT/target/clear-${arch}-unknown-none"
  local clanginc
  clanginc="$("$cc" -print-resource-dir)/include"

  # Same include set the ncurses library build uses (HAVE_CONFIG_H + the
  # generated/source ncurses headers), plus progs/ for clear_cmd.h and
  # tty_settings.h. -nostdinc with the clang + newlib sysroots, as elsewhere.
  local cppflags=(
    -DHAVE_CONFIG_H
    -I"$SRC/progs"
    -I"$WORK/ncurses" -I"$SRC/ncurses"
    -I"$WORK/include" -I"$SRC/include"
    -nostdinc -isystem "$clanginc" -isystem "$inc"
    -I"$ROOT/toolchain/newlib/libgloss/myos"
    -D_DEFAULT_SOURCE -D_GNU_SOURCE
  )

  local objs=()
  local extra=()
  local f o
  rm -rf "$objdir"
  mkdir -p "$objdir"
  echo "==> clear ($triple)"
  for f in "${CLEAR_SRCS[@]}"; do
    o="$objdir/${f}.o"
    echo "  CC ${f}.c"
    "$cc" -ffreestanding -fPIC -O2 -std=gnu99 "${cppflags[@]}" \
      -c "$SRC/progs/${f}.c" -o "$o"
    objs+=("$o")
  done

  # newlib's printf/scanf pull long-double / soft-float builtins on the
  # no-FPU arches; link the same helpers lua and vim do.
  if [[ "$arch" == "aarch64" ]]; then
    "$cc" -ffreestanding -fPIC -O2 -isystem "$inc" \
      -c "$ROOT/ports/sbase/trunctfdf2.c" -o "$objdir/trunctfdf2.o"
    extra+=("$objdir/trunctfdf2.o")
  elif [[ "$arch" == "riscv64" ]]; then
    "$cc" -ffreestanding -fPIC -O2 -isystem "$inc" \
      -c "$ROOT/ports/sbase/riscv64-softfloat.c" -o "$objdir/riscv64-softfloat.o"
    extra+=("$objdir/riscv64-softfloat.o" "$(myos_riscv64_softfloat)")
  fi

  echo "  LD clear ($triple)"
  "$ld" -pie --no-dynamic-linker --gc-sections -o "$out" \
    --entry=_start -z max-page-size=4096 \
    "$lib/crt0.o" "${objs[@]}" "${extra[@]+"${extra[@]}"}" -L"$lib" -L"$nclib" \
    --start-group -lncurses -lc -lm -lgloss -lg --end-group
  "${triple}-strip" -s "$out" 2>/dev/null || strip -s "$out" 2>/dev/null || true
  echo "clear -> $out"
  ls -lh "$out"
}

for arch in x86_64 aarch64 riscv64; do
  build_arch "$arch"
done

echo "$(myos_clear_version_hash)" >"$MYOS_CLEAR_VERSION"
echo "clear build ok"
