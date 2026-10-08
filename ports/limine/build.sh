#!/usr/bin/env bash
# Cross-build Limine's tool (limine.c of the pinned release) for the three
# arches with newlib + myos libgloss.
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$HERE/../.." && pwd)"
# shellcheck source=scripts/myos-c-userspace-lib.sh
source "$ROOT/scripts/myos-c-userspace-lib.sh"

if myos_limine_is_current; then
  echo "limine ELFs up to date"
  exit 0
fi

"$HERE/fetch.sh"
"$ROOT/toolchain/newlib/build.sh"
export PATH="$ROOT/target/newlib-bin:$PATH"
SRC="$ROOT/target/limine-src"

for arch in x86_64 aarch64 riscv64; do
  triple="${arch}-unknown-myos"
  prefix="$ROOT/target/newlib-${arch}"
  inc="$prefix/${triple}/include"
  lib="$prefix/${triple}/lib"
  objdir="$ROOT/target/limine-obj-${arch}"
  out="$ROOT/target/limine-${arch}-unknown-none"
  rm -rf "$objdir"
  mkdir -p "$objdir"
  echo "==> limine ($triple)"
  "${triple}-cc" -ffreestanding -fPIC -O2 -std=c99 -D_FILE_OFFSET_BITS=64 \
    -ffunction-sections -fdata-sections -isystem "$inc" -I"$SRC" \
    -c "$SRC/limine.c" -o "$objdir/limine.o"
  # printf of the tool's messages: the soft-float helpers newlib's printf
  # wants on aarch64 and riscv64, as for oksh.
  extra=()
  if [[ "$arch" == "aarch64" ]]; then
    "${triple}-cc" -ffreestanding -fPIC -O2 -isystem "$inc" \
      -c "$ROOT/ports/sbase/trunctfdf2.c" -o "$objdir/trunctfdf2.o"
    extra+=("$objdir/trunctfdf2.o")
  elif [[ "$arch" == "riscv64" ]]; then
    "${triple}-cc" -ffreestanding -fPIC -O2 -isystem "$inc" \
      -c "$ROOT/ports/sbase/riscv64-softfloat.c" -o "$objdir/riscv64-softfloat.o"
    extra+=("$objdir/riscv64-softfloat.o" "$(myos_riscv64_softfloat)")
  fi
  "${triple}-ld" -pie --no-dynamic-linker --gc-sections -o "$out" \
    --entry=_start -z max-page-size=4096 \
    "$lib/crt0.o" "$objdir/limine.o" "${extra[@]+"${extra[@]}"}" -L"$lib" \
    --start-group -lc -lgloss -lg --end-group
  "${triple}-strip" -s "$out" 2>/dev/null || strip -s "$out" 2>/dev/null || true
  echo "limine -> $out"
done

echo "$(myos_limine_version_hash)" >"$MYOS_LIMINE_VERSION"
