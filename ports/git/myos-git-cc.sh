#!/usr/bin/env bash
# Compile with myos clang wrappers; link with ld.lld + crt0 + zlib + newlib.
set -euo pipefail
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

CPP_EXTRA=(
  -nostdinc
  -isystem "$CLANG_RES"
  -isystem "$INC"
  -I"$ROOT/ports/git/include"
  -I"$ROOT/ports/git"
  -I"$ROOT/toolchain/newlib/libgloss/myos"
  -I"$ZLIB/include"
  -include myos_compat.h
  -D_DEFAULT_SOURCE
  -D_GNU_SOURCE
  -D_BSD_SOURCE
  -DNO_OPENSSL
)

# Force guest shell path after Makefile flags (append at end of argv).
SHELL_OVERRIDE=(
  -USHELL_PATH
  -DSHELL_PATH='"/bin/custom/sh"'
)

if [[ "$is_compile" -eq 1 ]]; then
  exec "$CC" -ffreestanding -fPIC -O2 -std=gnu99 \
    -ffunction-sections -fdata-sections \
    -Wno-unused-parameter -Wno-unused-variable -Wno-unused-function \
    -Wno-pointer-sign -Wno-missing-field-initializers \
    -Wno-macro-redefined \
    "${CPP_EXTRA[@]}" "$@" "${SHELL_OVERRIDE[@]}"
fi

# Link mode: parse -o and collect .o / .a / -l
out=""
objs=()
libs=()
prev=""
for a in "$@"; do
  if [[ "$prev" == "-o" ]]; then
    out="$a"
    prev=""
    continue
  fi
  case "$a" in
    -o) prev="-o" ;;
    -l*) libs+=("$a") ;;
    -L*) ;;
    -pie|-shared|-static|-rdynamic) ;;
    -Wl,*) ;;
    *.o|*.a) objs+=("$a") ;;
    -*) ;;
    *) objs+=("$a") ;;
  esac
done
[[ -n "$out" ]] || { echo "myos-git-cc: missing -o" >&2; exit 1; }

extra=()
if [[ -n "$EXTRA_OBJ" ]]; then
  # shellcheck disable=SC2206
  extra=($EXTRA_OBJ)
fi

link_libs=()
for l in "${libs[@]}"; do
  case "$l" in
    -lz) link_libs+=("$ZLIB/lib/libz.a") ;;
    -lpthread|-lrt|-ldl|-lresolv|-lnsl|-lsocket) ;;
    *) link_libs+=("$l") ;;
  esac
done

"$LD" -pie --no-dynamic-linker --gc-sections -o "$out" \
  --entry=_start -z max-page-size=4096 \
  "$LIB/crt0.o" "${objs[@]}" "${extra[@]}" \
  -L"$LIB" -L"$ZLIB/lib" \
  "${link_libs[@]}" \
  --start-group -lz -lc -lm -lgloss -lg --end-group

"${TRIPLE}-strip" -s "$out" 2>/dev/null || strip -s "$out" 2>/dev/null || true
