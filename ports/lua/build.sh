#!/usr/bin/env bash
# Cross-build the Lua 5.4 interpreter (lua) with newlib + libgloss.
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$HERE/../.." && pwd)"
# shellcheck source=scripts/myos-c-userspace-lib.sh
source "$ROOT/scripts/myos-c-userspace-lib.sh"
# shellcheck source=ports/lua/versions.env
source "$HERE/versions.env"

if myos_lua_is_current; then
  echo "lua ELFs up to date"
  exit 0
fi

"$ROOT/ports/lua/fetch.sh"
"$ROOT/toolchain/newlib/build.sh"
export PATH="$ROOT/target/newlib-bin:$PATH"

TARBALL="$ROOT/target/lua-${LUA_VERSION}.tar.gz"
WORK="$ROOT/target/lua-myos-build"

if [[ ! -d "$WORK" ]]; then
  tar -xzf "$TARBALL" -C "$ROOT/target"
  rm -rf "$WORK"
  mv "$ROOT/target/lua-${LUA_VERSION}" "$WORK"
fi

# Core library sources (from src/Makefile CORE_O) minus onelua-only bits.
CORE_SRCS=(
  lapi.c lauxlib.c lbaselib.c lcode.c lcorolib.c lctype.c ldblib.c
  ldebug.c ldo.c ldump.c lfunc.c lgc.c linit.c liolib.c llex.c
  lmathlib.c lmem.c loadlib.c lobject.c lopcodes.c loslib.c lparser.c
  lstate.c lstring.c lstrlib.c ltable.c ltablib.c ltm.c lundump.c
  lutf8lib.c lvm.c lzio.c
)
# Standalone interpreter driver.
LUA_SRCS=(lua.c)

link_prog() {
  local arch="$1"
  shift
  local objs=(${@+"$@"})
  local triple="${arch}-unknown-myos"
  local out="$ROOT/target/lua-${arch}-unknown-none"
  local prefix="$ROOT/target/newlib-${arch}"
  local lib="$prefix/${triple}/lib"
  local ld="${triple}-ld"
  local extra=()

  if [[ "$arch" == "aarch64" && -f "$ROOT/ports/sbase/trunctfdf2.c" ]]; then
    "${triple}-cc" -ffreestanding -fPIC -O2 -isystem "$prefix/${triple}/include" \
      -c "$ROOT/ports/sbase/trunctfdf2.c" -o "$ROOT/target/lua-obj-${arch}/trunctfdf2.o"
    extra+=("$ROOT/target/lua-obj-${arch}/trunctfdf2.o")
  elif [[ "$arch" == "riscv64" && -f "$ROOT/ports/sbase/riscv64-softfloat.c" ]]; then
    "${triple}-cc" -ffreestanding -fPIC -O2 -isystem "$prefix/${triple}/include" \
      -c "$ROOT/ports/sbase/riscv64-softfloat.c" -o "$ROOT/target/lua-obj-${arch}/riscv64-softfloat.o"
    extra+=("$ROOT/target/lua-obj-${arch}/riscv64-softfloat.o")
  fi

  echo "  LD lua ($triple)"
  "$ld" -pie --no-dynamic-linker --gc-sections -o "$out" \
    --entry=_start -z max-page-size=4096 \
    "$lib/crt0.o" "${objs[@]+"${objs[@]}"}" "${extra[@]+"${extra[@]}"}" \
    -L"$lib" \
    --start-group -lc -lm -lgloss -lg --end-group
  "${triple}-strip" -s "$out" 2>/dev/null || strip -s "$out" 2>/dev/null || true
  ls -lh "$out"
}

build_arch() {
  local arch="$1"
  local triple="${arch}-unknown-myos"
  local prefix="$ROOT/target/newlib-${arch}"
  local inc="$prefix/${triple}/include"
  local cc="${triple}-cc"
  local objdir="$ROOT/target/lua-obj-${arch}"
  local objs=()
  local f o
  local cflags=(
    -ffreestanding -fPIC -O2 -std=gnu99
    -I"$inc"
    -I"$ROOT/toolchain/newlib/libgloss/myos"
    -include "$HERE/myos_compat.h"
  )

  mkdir -p "$objdir"
  for f in "${CORE_SRCS[@]}" "${LUA_SRCS[@]}"; do
    o="$objdir/${f%.c}.o"
    objs+=("$o")
    if [[ ! -f "$o" || "$o" -ot "$WORK/src/$f" ]]; then
      echo "  CC $f ($triple)"
      "$cc" "${cflags[@]}" -c "$WORK/src/$f" -o "$o"
    fi
  done
  link_prog "$arch" "${objs[@]}"
}

build_arch x86_64
build_arch aarch64
build_arch riscv64

echo "$(myos_lua_version_hash)" >"$MYOS_LUA_VERSION"
echo "lua build ok ($LUA_VERSION)"
