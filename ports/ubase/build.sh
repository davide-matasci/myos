#!/usr/bin/env bash
# Cross-build ubase getty + login with newlib + myos libgloss.
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$HERE/../.." && pwd)"
# shellcheck source=scripts/myos-c-userspace-lib.sh
source "$ROOT/scripts/myos-c-userspace-lib.sh"

if myos_ubase_is_current; then
  echo "ubase ELFs up to date"
  exit 0
fi

"$ROOT/ports/ubase/prepare.sh"
"$ROOT/toolchain/newlib/build.sh"
export PATH="$ROOT/target/newlib-bin:$PATH"

WORK="$ROOT/target/ubase-myos-build"
BINS_FILE="$ROOT/ports/ubase/bins.txt"
mapfile -t UBASE_BINS <"$BINS_FILE"
MYOS="$ROOT/ports/ubase"

CPPFLAGS=(
  -D_DEFAULT_SOURCE
  -D_GNU_SOURCE
  -D_BSD_SOURCE
  -D_XOPEN_SOURCE=700
  -I"$WORK"
  -I"$MYOS"
  -I"$ROOT/toolchain/newlib/libgloss/myos"
  -include myos_compat.h
  -include sys/myos_extra.h
  -Wno-implicit-function-declaration
)

LIBUTIL_SRCS=(
  libutil/eprintf.c
  libutil/ealloc.c
  libutil/strlcpy.c
  libutil/strlcat.c
)

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
  local out="$ROOT/target/ubase-${out_name}-${arch}-unknown-none"
  local prefix="$ROOT/target/newlib-${arch}"
  local lib="$prefix/${triple}/lib"
  local ld="${triple}-ld"

  "$ld" -pie --no-dynamic-linker --gc-sections -o "$out" \
    --entry=_start -z max-page-size=4096 \
    "$lib/crt0.o" "${objs[@]}" -L"$lib" \
    --start-group -lc -lgloss -lg --end-group
  "${triple}-strip" -s "$out" 2>/dev/null || strip -s "$out" 2>/dev/null || true
}

build_arch() {
  local arch="$1"
  local triple="${arch}-unknown-myos"
  local prefix="$ROOT/target/newlib-${arch}"
  local inc="$prefix/${triple}/include"
  local cc="${triple}-cc"
  local objdir="$ROOT/target/ubase-obj-${arch}"
  local manifest="$ROOT/target/ubase-manifest-${arch}.txt"
  local built=0
  local failed=()
  local extra=()

  rm -rf "$objdir"
  mkdir -p "$objdir"
  : >"$manifest"

  local util_objs=()
  local src base obj
  for src in "${LIBUTIL_SRCS[@]}"; do
    base="$(basename "$src" .c)"
    obj="$objdir/libutil-${base}.o"
    compile "$cc" "$inc" "$WORK/$src" "$obj"
    util_objs+=("$obj")
  done

  compile "$cc" "$inc" "$MYOS/pw_check.c" "$objdir/pw_check.o"
  util_objs+=("$objdir/pw_check.o")

  if [[ "$arch" == "aarch64" ]]; then
    if [[ -f "$ROOT/ports/sbase/trunctfdf2.c" ]]; then
      compile "$cc" "$inc" "$ROOT/ports/sbase/trunctfdf2.c" "$objdir/trunctfdf2.o"
      extra=("$objdir/trunctfdf2.o")
    fi
  elif [[ "$arch" == "riscv64" ]]; then
    if [[ -f "$ROOT/ports/sbase/riscv64-softfloat.c" ]]; then
      compile "$cc" "$inc" "$ROOT/ports/sbase/riscv64-softfloat.c" "$objdir/riscv64-softfloat.o"
      extra=("$objdir/riscv64-softfloat.o")
    fi
  fi

  local name
  for name in "${UBASE_BINS[@]}"; do
    [[ -n "$name" ]] || continue
    obj="$objdir/prog-${name}.o"
    echo "==> ubase-${name} ($triple)"
    if ! compile "$cc" "$inc" "$WORK/${name}.c" "$obj"; then
      failed+=("$name:compile")
      continue
    fi
    if ! link_prog "$name" "$arch" "${util_objs[@]}" "$obj" "${extra[@]}"; then
      failed+=("$name:link")
      rm -f "$ROOT/target/ubase-${name}-${arch}-unknown-none"
      continue
    fi
    echo "${name}:$ROOT/target/ubase-${name}-${arch}-unknown-none" >>"$manifest"
    built=$((built + 1))
  done

  echo "ubase ${arch}: built ${built}/$((${#UBASE_BINS[@]})) (${#failed[@]} failed)"
  if ((${#failed[@]} > 0)); then
    printf '  failed: %s\n' "${failed[@]}" >&2
  fi
  if ((built == 0)); then
    echo "error: no ubase ELFs built for ${arch}" >&2
    exit 1
  fi
}

for arch in x86_64 aarch64 riscv64; do
  build_arch "$arch"
done

echo "$(myos_ubase_version_hash)" >"$MYOS_UBASE_VERSION"
