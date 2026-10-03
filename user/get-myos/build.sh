#!/usr/bin/env bash
# Cross-build get-myos (the package installer) for the three arches with
# newlib + myos libgloss and the zlib port (packages are gzip tars).
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$HERE/../.." && pwd)"
# shellcheck source=scripts/myos-c-userspace-lib.sh
source "$ROOT/scripts/myos-c-userspace-lib.sh"

"$ROOT/toolchain/newlib/build.sh"
"$ROOT/ports/zlib/build.sh"
export PATH="$ROOT/target/newlib-bin:$PATH"
myos_ensure_llvm_bin

if myos_get_myos_is_current; then
  echo "get-myos up to date"
  exit 0
fi

for arch in x86_64 aarch64 riscv64; do
  triple="${arch}-unknown-myos"
  out="$ROOT/target/get-myos-${arch}-unknown-none"
  nl="$ROOT/target/newlib-${arch}/${triple}"
  zl="$ROOT/target/zlib-${arch}"
  cc="${triple}-cc"
  objs=()
  echo "==> get-myos ($triple)"
  for src in get-myos pkgtools; do
    obj="$ROOT/target/get-myos-${src}-${arch}.o"
    "$cc" -ffreestanding -fPIC -O2 -isystem "$nl/include" -I"$zl/include" \
      -c "$HERE/$src.c" -o "$obj"
    objs+=("$obj")
  done
  # No printf (fputs only), so no soft-float helpers, like get-alpine.
  ld.lld -pie --no-dynamic-linker -o "$out" \
    --entry=_start -z max-page-size=4096 \
    "$nl/lib/crt0.o" "${objs[@]}" "$zl/lib/libz.a" -L"$nl/lib" \
    --start-group -lc -lgloss -lg --end-group
  echo "get-myos -> $out"
done
echo "$(myos_get_myos_version_hash)" >"$MYOS_GET_MYOS_VERSION"
