#!/bin/sh
# Cross-build GNU make with newlib + myos libgloss (3 arches).
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
ROOT="$(cd "$HERE/../.." && pwd)"
# shellcheck source=scripts/myos-c-userspace-lib.sh
. "$ROOT/scripts/myos-c-userspace-lib.sh"

if myos_make_is_current; then
  echo "make ELFs up to date"
  exit 0
fi

"$HERE/prepare.sh"
"$ROOT/toolchain/newlib/build.sh"
export PATH="$ROOT/target/newlib-bin:$PATH"

WORK="$ROOT/target/make-myos-build"

# POSIX build: everything except amiga/vms/w32/remote-cstms/guile/load-api.
# load.c still compiles (it becomes stubs without MAKE_LOAD).
MAKE_SRCS="
  ar.c arscan.c commands.c default.c dir.c expand.c file.c function.c
  getopt.c getopt1.c hash.c implicit.c job.c load.c loadapi.c main.c
  misc.c output.c posixos.c read.c remake.c remote-stub.c rule.c
  shuffle.c signame.c strcache.c variable.c version.c vpath.c
  glob.c fnmatch.c
"

# cc_cpp SRC OBJ FLAG...: build_arch's compiler with the FLAGs, then the
# preprocessor's, SRC into OBJ.
cc_cpp() {
  _cc_src="$1" _cc_obj="$2"
  shift 2
  "$cc" "$@" \
    -D_DEFAULT_SOURCE \
    -D_GNU_SOURCE \
    -I"$WORK" \
    -include config.h \
    -include myos_compat.h \
    -c "$_cc_src" -o "$_cc_obj"
}

link_prog() (
  arch="$1"
  shift
  triple="${arch}-unknown-myos"
  out="$ROOT/target/make-${arch}-unknown-none"
  lib="$ROOT/target/newlib-${arch}/${triple}/lib"
  ld="${triple}-ld"

  "$ld" -pie --no-dynamic-linker --gc-sections -o "$out" \
    --entry=_start -z max-page-size=4096 \
    "$lib/crt0.o" "$@" -L"$lib" \
    --start-group -lc -lgloss -lg --end-group
  true
  echo "make -> $out"
  if command -v llvm-size >/dev/null 2>&1; then
    llvm-size "$out" || true
  elif command -v size >/dev/null 2>&1; then
    size "$out" || true
  fi
)

build_arch() (
  arch="$1"
  triple="${arch}-unknown-myos"
  prefix="$ROOT/target/newlib-${arch}"
  inc="$prefix/${triple}/include"
  cc="${triple}-cc"
  objdir="$ROOT/target/make-obj-${arch}"
  set --

  rm -rf "$objdir"
  mkdir -p "$objdir"

  echo "==> make ($triple)"
  for src in $MAKE_SRCS; do
    base="$(basename "$src" .c)"
    obj="$objdir/${base}.o"
    cc_cpp "$WORK/$src" "$obj" -ffreestanding -fPIC -O2 -std=gnu99 \
      -ffunction-sections -fdata-sections \
      -Wno-unused-parameter -Wno-unused-variable -Wno-unused-function \
      -Wno-pointer-sign -Wno-missing-field-initializers \
      -isystem "$inc"
    set -- "$@" "$obj"
  done

  if [ "$arch" = "aarch64" ] || [ "$arch" = "riscv64" ]; then
    "$cc" -ffreestanding -fPIC -O2 -isystem "$inc" \
      -c "$ROOT/ports/sbase/trunctfdf2.c" -o "$objdir/trunctfdf2.o"
    set -- "$@" "$objdir/trunctfdf2.o"
  fi

  # myos gap shims (dup, vfork, getloadavg) + guile stubs.
  cc_cpp "$HERE/myos_shims.c" "$objdir/myos_shims.o" \
    -ffreestanding -fPIC -O2 -std=gnu99 -isystem "$inc"
  cc_cpp "$WORK/guile.c" "$objdir/guile.o" \
    -ffreestanding -fPIC -O2 -std=gnu99 -isystem "$inc"
  set -- "$@" "$objdir/myos_shims.o" "$objdir/guile.o"

  link_prog "$arch" "$@"
)

for arch in x86_64 aarch64 riscv64; do
  build_arch "$arch"
done

echo "$(myos_make_version_hash)" >"$MYOS_MAKE_VERSION"
