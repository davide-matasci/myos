#!/bin/sh
# Compile with myos clang wrappers; link with ld.lld + crt0 + zlib + newlib.
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
ARCH="${MYOS_GIT_ARCH:?MYOS_GIT_ARCH unset}"
ROOT="${MYOS_GIT_ROOT:?MYOS_GIT_ROOT unset}"
TRIPLE="${ARCH}-unknown-myos"
CC="${TRIPLE}-cc"
LD="${TRIPLE}-ld"
PREFIX="$ROOT/target/newlib-${ARCH}"
INC="$PREFIX/${TRIPLE}/include"
LIB="$PREFIX/${TRIPLE}/lib"
ZLIB="$ROOT/target/zlib-${ARCH}"
EXTRA_OBJ="${MYOS_GIT_EXTRA_OBJ:-}"
CLANG_RES="$(clang -print-resource-dir)/include"

is_compile=0
for a in "$@"; do
  case "$a" in
    -c|-E|-S|-M|-MM|-MD|-MMD) is_compile=1; break ;;
  esac
done

# Force guest shell path after Makefile flags (append at end of argv).
if [ "$is_compile" -eq 1 ]; then
  exec "$CC" -ffreestanding -fPIC -O2 -std=gnu99 \
    -ffunction-sections -fdata-sections \
    -Wno-unused-parameter -Wno-unused-variable -Wno-unused-function \
    -Wno-pointer-sign -Wno-missing-field-initializers \
    -Wno-macro-redefined \
    -nostdinc \
    -isystem "$CLANG_RES" \
    -isystem "$INC" \
    -I"$HERE/include" \
    -I"$HERE" \
    -I"$ROOT/toolchain/newlib/libgloss/myos" \
    -I"$ZLIB/include" \
    -include myos_compat.h \
    -D_DEFAULT_SOURCE \
    -D_GNU_SOURCE \
    -D_BSD_SOURCE \
    -DNO_OPENSSL \
    "$@" \
    -USHELL_PATH \
    -DSHELL_PATH='"/bin/custom/sh"'
fi

# Link mode: parse -o and collect .o / .a / -l. The objects are appended to
# the positional parameters, behind the arguments, which are dropped after
# the loop; the -l in a string (no spaces in them).
out=""
nargs=$#
libs=""
prev=""
for a in "$@"; do
  if [ "$prev" = "-o" ]; then
    out="$a"
    prev=""
    continue
  fi
  case "$a" in
    -o) prev="-o" ;;
    -l*) libs="$libs $a" ;;
    -L*) ;;
    -pie|-shared|-static|-rdynamic) ;;
    -Wl,*) ;;
    *.o|*.a) set -- "$@" "$a" ;;
    -*) ;;
    *) set -- "$@" "$a" ;;
  esac
done
shift "$nargs"
[ -n "$out" ] || { echo "myos-git-cc: missing -o" >&2; exit 1; }

# The link's arguments from crt0.o to the libraries: the objects, the
# extra ones (a list split on purpose), then the libraries.
# shellcheck disable=SC2086
set -- "$LIB/crt0.o" "$@" $EXTRA_OBJ \
  -L"$LIB" -L"$ZLIB/lib"

# shellcheck disable=SC2086
for l in $libs; do
  case "$l" in
    -lz) set -- "$@" "$ZLIB/lib/libz.a" ;;
    -lpthread|-lrt|-ldl|-lresolv|-lnsl|-lsocket) ;;
    *) set -- "$@" "$l" ;;
  esac
done

"$LD" -pie --no-dynamic-linker --gc-sections -o "$out" \
  --entry=_start -z max-page-size=4096 \
  "$@" \
  --start-group -lz -lc -lm -lgloss -lg --end-group

"${TRIPLE}-strip" -s "$out" 2>/dev/null || strip -s "$out" 2>/dev/null || true
