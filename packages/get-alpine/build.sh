#!/usr/bin/env bash
# Cross-build get-alpine for the three arches with newlib + myos libgloss,
# the zlib port (Alpine's packages and indexes are gzip tars) and the
# download/tar/gzip code it shares with get-myos (user/get-myos/pkgtools.c).
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$HERE/../.." && pwd)"
# shellcheck source=scripts/myos-c-userspace-lib.sh
source "$ROOT/scripts/myos-c-userspace-lib.sh"

"$ROOT/toolchain/newlib/build.sh"
"$ROOT/ports/zlib/build.sh"
export PATH="$ROOT/target/newlib-bin:$PATH"
myos_ensure_llvm_bin

if myos_get_alpine_is_current; then
  echo "get-alpine up to date"
  exit 0
fi

for arch in x86_64 aarch64 riscv64; do
  triple="${arch}-unknown-myos"
  out="$ROOT/target/get-alpine-${arch}-unknown-none"
  nl="$ROOT/target/newlib-${arch}/${triple}"
  zl="$ROOT/target/zlib-${arch}"
  cc="${triple}-cc"
  echo "==> get-alpine ($triple)"
  "$cc" -ffreestanding -fPIC -O2 -isystem "$nl/include" -I"$zl/include" -I"$ROOT/user/get-myos" \
    -c "$HERE/get-alpine.c" -o "$ROOT/target/get-alpine-${arch}.o"
  "$cc" -ffreestanding -fPIC -O2 -isystem "$nl/include" -I"$zl/include" \
    -c "$ROOT/user/get-myos/pkgtools.c" -o "$ROOT/target/get-alpine-pkgtools-${arch}.o"
  # No printf (fputs only), so no soft-float helpers, like get-myos.
  ld.lld -pie --no-dynamic-linker -o "$out" \
    --entry=_start -z max-page-size=4096 \
    "$nl/lib/crt0.o" "$ROOT/target/get-alpine-${arch}.o" "$ROOT/target/get-alpine-pkgtools-${arch}.o" \
    "$zl/lib/libz.a" -L"$nl/lib" \
    --start-group -lc -lgloss -lg --end-group
  echo "get-alpine -> $out"
done
echo "$(myos_get_alpine_version_hash)" >"$MYOS_GET_ALPINE_VERSION"
