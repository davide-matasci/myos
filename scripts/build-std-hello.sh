#!/usr/bin/env bash
# Build std example ELFs for CI (small smoke binaries).
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# shellcheck source=scripts/myos-sysroot-lib.sh
source "$ROOT/scripts/myos-sysroot-lib.sh"
"$ROOT/scripts/fetch-sysroot.sh"
mkdir -p "$ROOT/target"

if myos_std_hello_is_current; then
  echo "std example ELFs up to date"
  exit 0
fi

build_example() {
  local name="$1" manifest="$2" bin="$3" triple="$4"
  local target_dir="$ROOT/target/std-${name}-build-${triple}"
  echo "==> std-${name} ($triple)"
  myos_cargo_build_app "$triple" release "$target_dir" "$manifest" "$bin"
  cp "$target_dir/${triple}/release/${bin}" "$ROOT/target/std-${name}-${triple}"
  echo "std-${name} -> $ROOT/target/std-${name}-${triple}"
}
for triple in "${MYOS_USER_TRIPLES[@]}"; do
  build_example hello "$ROOT/std/examples/hello/Cargo.toml" std-hello "$triple"
  build_example cat "$ROOT/std/examples/cat/Cargo.toml" std-cat "$triple"
  build_example echo "$ROOT/std/examples/echo/Cargo.toml" std-echo "$triple"
  build_example bigalloc "$ROOT/std/examples/bigalloc/Cargo.toml" bigalloc "$triple"
done
echo "$(myos_std_hello_version_hash)" >"$MYOS_STD_HELLO_VERSION"
