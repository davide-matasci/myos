#!/usr/bin/env bash
# Cross-build a small sbase subset (echo, cat, true) with newlib + libgloss.
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
"$ROOT/scripts/prepare-sbase-myos.sh"
"$ROOT/scripts/build-newlib.sh"
export PATH="$ROOT/target/newlib-bin:$PATH"

SBASE="$ROOT/target/sbase-src"
WORK="$ROOT/target/sbase-myos-build"
MYOS="$ROOT/scripts/sbase-myos"
CPPFLAGS=(-I"$MYOS" -I"$SBASE" -D_POSIX_C_SOURCE=200809L)

compile() {
  local cc="$1"
  local inc="$2"
  local src="$3"
  local out="$4"
  "$cc" -ffreestanding -fPIC -O2 -isystem "$inc" "${CPPFLAGS[@]}" -c "$src" -o "$out"
}

link_prog() {
  local name="$1"
  local arch="$2"
  shift 2
  local objs=("$@")
  local triple="${arch}-unknown-myos"
  local out="$ROOT/target/sbase-${name}-${arch}-unknown-none"
  local prefix="$ROOT/target/newlib-${arch}"
  local lib="$prefix/${triple}/lib"
  local ld="${triple}-ld"

  echo "==> sbase-${name} ($triple / newlib)"
  "$ld" -pie --no-dynamic-linker -o "$out" \
    --entry=_start -z max-page-size=4096 \
    "$lib/crt0.o" "${objs[@]}" -L"$lib" \
    --start-group -lc -lgloss -lg --end-group
  echo "sbase-${name} -> $out"
}

build_arch() {
  local arch="$1"
  local triple="${arch}-unknown-myos"
  local prefix="$ROOT/target/newlib-${arch}"
  local inc="$prefix/${triple}/include"
  local cc="${triple}-cc"
  local objdir="$ROOT/target/sbase-obj-${arch}"
  mkdir -p "$objdir"

  compile "$cc" "$inc" "$MYOS/eprintf.c" "$objdir/eprintf.o"
  compile "$cc" "$inc" "$SBASE/libutil/writeall.c" "$objdir/writeall.o"
  compile "$cc" "$inc" "$SBASE/libutil/concat.c" "$objdir/concat.o"
  compile "$cc" "$inc" "$MYOS/putword.c" "$objdir/putword.o"

  local echo_objs=(
    "$objdir/eprintf.o"
    "$objdir/writeall.o"
    "$objdir/putword.o"
    "$objdir/echo.o"
  )
  compile "$cc" "$inc" "$WORK/echo.c" "$objdir/echo.o"

  local cat_objs=(
    "$objdir/eprintf.o"
    "$objdir/writeall.o"
    "$objdir/concat.o"
    "$objdir/cat.o"
  )
  compile "$cc" "$inc" "$SBASE/cat.c" "$objdir/cat.o"

  local true_objs=("$objdir/true.o")
  compile "$cc" "$inc" "$SBASE/true.c" "$objdir/true.o"

  if [[ "$arch" == "aarch64" ]]; then
    compile "$cc" "$inc" "$MYOS/trunctfdf2.c" "$objdir/trunctfdf2.o"
    echo_objs+=("$objdir/trunctfdf2.o")
    cat_objs+=("$objdir/trunctfdf2.o")
  fi

  link_prog echo "$arch" "${echo_objs[@]}"
  link_prog cat "$arch" "${cat_objs[@]}"
  link_prog true "$arch" "${true_objs[@]}"
}

for arch in x86_64 aarch64; do
  build_arch "$arch"
done
