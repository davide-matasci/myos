#!/bin/sh
# Cross-compile C smoke ELFs with newlib + myos libgloss.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
# shellcheck source=scripts/myos-c-userspace-lib.sh
. "$ROOT/scripts/myos-c-userspace-lib.sh"

pack_socket_smoke_aliases() (
  # CI packs via existing `target/coreutils-*` glob (workflow edits need workflow scope).
  # Keep canonical and pack-alias names in sync either way (same as curl aliases).
  for arch in $MYOS_ARCHES; do
    src="$ROOT/target/c-socket_smoke-${arch}-unknown-none"
    alias="$ROOT/target/coreutils-c-socket_smoke-${arch}-unknown-none"
    if [ -f "$src" ]; then
      cp "$src" "$alias"
    elif [ -f "$alias" ]; then
      cp "$alias" "$src"
    fi
  done
)

if myos_c_hello_is_current; then
  echo "c-hello ELFs up to date"
  pack_socket_smoke_aliases
  exit 0
fi

"$ROOT/toolchain/newlib/build.sh"
export PATH="$ROOT/target/newlib-bin:$PATH"
myos_ensure_llvm_bin

link_prog() (
  name="$1"
  src="$2"
  arch="$3"
  triple="${arch}-unknown-myos"
  out="$ROOT/target/c-${name}-${arch}-unknown-none"
  prefix="$ROOT/target/newlib-${arch}"
  inc="$prefix/${triple}/include"
  lib="$prefix/${triple}/lib"
  obj="$ROOT/target/c-${name}-${arch}.o"
  cc="${triple}-cc"
  # The extra objects of the link: this subshell's arguments from here on.
  set --

  echo "==> c-${name} ($triple / newlib)"
  "$cc" -ffreestanding -fPIC -O2 -isystem "$inc" -c "$src" -o "$obj"

  # long-double helpers needed when libgloss pulls snprintf (aarch64/riscv64).
  if [ "$arch" = "aarch64" ] || [ "$arch" = "riscv64" ]; then
    tf="$ROOT/target/c-${name}-${arch}-trunctfdf2.o"
    "$cc" -ffreestanding -fPIC -O2 -isystem "$inc" -c "$ROOT/ports/sbase/trunctfdf2.c" -o "$tf"
    set -- "$tf"
  fi

  ld.lld -pie --no-dynamic-linker -o "$out" \
    --entry=_start -z max-page-size=4096 \
    "$lib/crt0.o" "$obj" "$@" -L"$lib" --start-group -lc -lgloss -lg --end-group
  echo "c-${name} -> $out"
)

for arch in $MYOS_ARCHES; do
  link_prog hello "$ROOT/user/c/hello.c" "$arch"
  link_prog socket_smoke "$ROOT/user/c/socket_smoke.c" "$arch"
done

# No stamp on myos: what is built there is current (myos_c_hello_is_current).
myos_self_host || echo "$(myos_c_hello_version_hash)" >"$MYOS_C_HELLO_VERSION"
pack_socket_smoke_aliases
