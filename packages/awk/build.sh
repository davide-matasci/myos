#!/bin/sh
# Cross-build awk (the one true awk) with newlib + libgloss.
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
ROOT="$(cd "$HERE/../.." && pwd)"
# shellcheck source=scripts/myos-c-userspace-lib.sh
. "$ROOT/scripts/myos-c-userspace-lib.sh"
# shellcheck source=packages/awk/versions.env
. "$HERE/versions.env"

if myos_awk_is_current; then
  echo "awk ELFs up to date"
  exit 0
fi
# The grammar is yacc's to turn into C (the release has no generated
# parser), and bison is the one its makefile names.
if ! command -v bison >/dev/null 2>&1; then
  echo "error: bison not found: apt install bison" >&2
  exit 1
fi

"$HERE/fetch.sh"
"$ROOT/toolchain/newlib/build.sh"
export PATH="$ROOT/target/newlib-bin:$PATH"

TARBALL="$ROOT/target/awk-${AWK_VERSION}.tar.gz"
WORK="$ROOT/target/awk-myos-build"

rm -rf "$WORK"
mkdir -p "$WORK"
tar -xzf "$TARBALL" -C "$WORK" --strip-components=1

# What its makefile generates, on the build machine: the parser, and the
# table of the functions that run each node (maketab reads the parser's
# token numbers).
(
  cd "$WORK"
  bison -d -Wnone awkgram.y
  ${HOSTCC:-cc} -O2 maketab.c -o maketab
  ./maketab awkgram.tab.h >proctab.c
)
SRCS="awkgram.tab.c b.c main.c parse.c proctab.c tran.c lib.c run.c lex.c"

build_arch() (
  arch="$1"
  triple="${arch}-unknown-myos"
  prefix="$ROOT/target/newlib-${arch}"
  lib="$prefix/${triple}/lib"
  objdir="$ROOT/target/awk-obj-${arch}"
  out="$ROOT/target/awk-${arch}-unknown-none"
  set --

  rm -rf "$objdir"
  mkdir -p "$objdir"
  for f in $SRCS; do
    o="$objdir/${f%.c}.o"
    set -- "$@" "$o"
    echo "  CC $f ($triple)"
    "${triple}-cc" -O2 \
      -isystem "$prefix/${triple}/include" \
      -I"$ROOT/toolchain/newlib/libgloss/myos" \
      -c "$WORK/$f" -o "$o"
  done
  # The long-double conversions newlib's printf wants, which these arches
  # have no instruction for.
  if [ "$arch" != x86_64 ]; then
    "${triple}-cc" -ffreestanding -fPIC -O2 -isystem "$prefix/${triple}/include" \
      -c "$ROOT/ports/sbase/trunctfdf2.c" -o "$objdir/trunctfdf2.o"
    set -- "$@" "$objdir/trunctfdf2.o"
  fi
  echo "  LD awk ($triple)"
  "${triple}-ld" -pie --no-dynamic-linker --gc-sections -o "$out" \
    --entry=_start -z max-page-size=4096 \
    "$lib/crt0.o" "$@" \
    -L"$lib" \
    --start-group -lc -lm -lgloss -lg --end-group
  "${triple}-strip" -s "$out"
  ls -lh "$out"
)

build_arch x86_64
build_arch aarch64
build_arch riscv64

echo "$(myos_awk_version_hash)" >"$MYOS_AWK_VERSION"
echo "awk build ok ($AWK_VERSION)"
