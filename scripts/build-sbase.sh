#!/usr/bin/env bash
# Cross-build a sbase subset with newlib + myos libgloss.
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# shellcheck source=scripts/myos-c-userspace-lib.sh
source "$ROOT/scripts/myos-c-userspace-lib.sh"

if myos_sbase_is_current; then
  echo "sbase ELFs up to date"
  exit 0
fi

"$ROOT/scripts/prepare-sbase-myos.sh"
"$ROOT/scripts/build-newlib.sh"
export PATH="$ROOT/target/newlib-bin:$PATH"

SBASE="$ROOT/target/sbase-src"
WORK="$ROOT/target/sbase-myos-build"
MYOS="$ROOT/scripts/sbase-myos"
CPPFLAGS=(-I"$SBASE" -include sys/myos_extra.h -D_POSIX_C_SOURCE=200809L)

compile() {
  local cc="$1"
  local inc="$2"
  local src="$3"
  local out="$4"
  "$cc" -ffreestanding -fPIC -O2 -isystem "$inc" "${CPPFLAGS[@]}" -c "$src" -o "$out"
}

link_prog() {
  local out_name="$1"
  local arch="$2"
  shift 2
  local objs=("$@")
  local triple="${arch}-unknown-myos"
  local out="$ROOT/target/sbase-${out_name}-${arch}-unknown-none"
  local prefix="$ROOT/target/newlib-${arch}"
  local lib="$prefix/${triple}/lib"
  local ld="${triple}-ld"

  echo "==> sbase-${out_name} ($triple / newlib)"
  "$ld" -pie --no-dynamic-linker -o "$out" \
    --entry=_start -z max-page-size=4096 \
    "$lib/crt0.o" "${objs[@]}" -L"$lib" \
    --start-group -lc -lgloss -lg --end-group
  echo "sbase-${out_name} -> $out"
}

build_arch() {
  local arch="$1"
  local triple="${arch}-unknown-myos"
  local prefix="$ROOT/target/newlib-${arch}"
  local inc="$prefix/${triple}/include"
  local cc="${triple}-cc"
  local objdir="$ROOT/target/sbase-obj-${arch}"
  mkdir -p "$objdir"

  compile "$cc" "$inc" "$SBASE/libutil/eprintf.c" "$objdir/eprintf.o"
  compile "$cc" "$inc" "$SBASE/libutil/fshut.c" "$objdir/fshut.o"
  compile "$cc" "$inc" "$SBASE/libutil/writeall.c" "$objdir/writeall.o"
  compile "$cc" "$inc" "$SBASE/libutil/concat.c" "$objdir/concat.o"
  compile "$cc" "$inc" "$SBASE/libutil/putword.c" "$objdir/putword.o"

  local util_common=(
    "$objdir/eprintf.o"
    "$objdir/fshut.o"
    "$objdir/writeall.o"
  )

  local extra=()
  if [[ "$arch" == "aarch64" ]]; then
    compile "$cc" "$inc" "$MYOS/trunctfdf2.c" "$objdir/trunctfdf2.o"
    extra=("$objdir/trunctfdf2.o")
  fi

  compile "$cc" "$inc" "$WORK/echo.c" "$objdir/echo.o"
  link_prog echo "$arch" "${util_common[@]}" "$objdir/putword.o" "$objdir/echo.o" "${extra[@]}"

  compile "$cc" "$inc" "$SBASE/cat.c" "$objdir/cat.o"
  link_prog cat "$arch" "${util_common[@]}" "$objdir/concat.o" "$objdir/cat.o" "${extra[@]}"

  compile "$cc" "$inc" "$SBASE/true.c" "$objdir/true.o"
  link_prog true "$arch" "$objdir/true.o"

  compile "$cc" "$inc" "$SBASE/false.c" "$objdir/false.o"
  link_prog false "$arch" "$objdir/false.o"

  compile "$cc" "$inc" "$SBASE/libutil/ealloc.c" "$objdir/ealloc.o"
  compile "$cc" "$inc" "$SBASE/libutil/reallocarray.c" "$objdir/reallocarray.o"
  compile "$cc" "$inc" "$SBASE/libutil/human.c" "$objdir/human.o"
  compile "$cc" "$inc" "$SBASE/libutf/runetype.c" "$objdir/runetype.o"
  compile "$cc" "$inc" "$SBASE/libutf/rune.c" "$objdir/rune.o"
  compile "$cc" "$inc" "$SBASE/libutf/iscntrlrune.c" "$objdir/iscntrlrune.o"
  compile "$cc" "$inc" "$SBASE/libutf/isprintrune.c" "$objdir/isprintrune.o"
  compile "$cc" "$inc" "$SBASE/libutf/utf.c" "$objdir/utf.o"
  compile "$cc" "$inc" "$WORK/ls.c" "$objdir/ls.o"
  link_prog ls "$arch" "${util_common[@]}" "$objdir/ealloc.o" "$objdir/reallocarray.o" \
    "$objdir/human.o" "$objdir/runetype.o" "$objdir/rune.o" "$objdir/iscntrlrune.o" \
    "$objdir/isprintrune.o" "$objdir/utf.o" "$objdir/ls.o" "${extra[@]}"

  compile "$cc" "$inc" "$WORK/pwd.c" "$objdir/pwd.o"
  link_prog pwd "$arch" "${util_common[@]}" "$objdir/pwd.o" "${extra[@]}"

  compile "$cc" "$inc" "$WORK/basename.c" "$objdir/basename.o"
  link_prog basename "$arch" "${util_common[@]}" "$objdir/basename.o" "${extra[@]}"

  compile "$cc" "$inc" "$WORK/dirname.c" "$objdir/dirname.o"
  link_prog dirname "$arch" "${util_common[@]}" "$objdir/dirname.o" "${extra[@]}"
}

for arch in x86_64 aarch64 riscv64; do
  build_arch "$arch"
done

echo "$(myos_sbase_version_hash)" >"$MYOS_SBASE_VERSION"
