#!/usr/bin/env bash
# Cross-compile C smoke ELFs with newlib + myos libgloss.
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# shellcheck source=scripts/myos-c-userspace-lib.sh
source "$ROOT/scripts/myos-c-userspace-lib.sh"

if myos_c_hello_is_current; then
  echo "c-hello ELFs up to date"
  exit 0
fi

"$ROOT/toolchain/newlib/build.sh"
export PATH="$ROOT/target/newlib-bin:$PATH"

link_prog() {
  local name="$1"
  local src="$2"
  local arch="$3"
  local triple="${arch}-unknown-myos"
  local out="$ROOT/target/c-${name}-${arch}-unknown-none"
  local prefix="$ROOT/target/newlib-${arch}"
  local inc="$prefix/${triple}/include"
  local lib="$prefix/${triple}/lib"
  local obj="$ROOT/target/c-${name}-${arch}.o"
  local cc="${triple}-cc"
  local extra=()

  echo "==> c-${name} ($triple / newlib)"
  "$cc" -ffreestanding -fPIC -O2 -isystem "$inc" -c "$src" -o "$obj"

  # soft-float long-double helpers needed when libgloss pulls snprintf (aarch64/riscv).
  if [[ "$arch" == "aarch64" || "$arch" == "riscv64" ]]; then
    local tf="$ROOT/target/c-${name}-${arch}-trunctfdf2.o"
    "$cc" -ffreestanding -fPIC -O2 -isystem "$inc" -c "$ROOT/ports/sbase/trunctfdf2.c" -o "$tf"
    extra+=("$tf")
  fi

  ld.lld -pie --no-dynamic-linker -o "$out" \
    --entry=_start -z max-page-size=4096 \
    "$lib/crt0.o" "$obj" "${extra[@]}" -L"$lib" --start-group -lc -lgloss -lg --end-group
  echo "c-${name} -> $out"
}

for arch in x86_64 aarch64 riscv64; do
  link_prog hello "$ROOT/c/hello.c" "$arch"
  link_prog socket_smoke "$ROOT/c/socket_smoke.c" "$arch"
done

echo "$(myos_c_hello_version_hash)" >"$MYOS_C_HELLO_VERSION"
