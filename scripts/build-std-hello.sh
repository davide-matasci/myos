#!/usr/bin/env bash
# Build std-hello for every myos userspace triple using the prebuilt sysroot.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# shellcheck source=scripts/myos-sysroot-lib.sh
source "$ROOT/scripts/myos-sysroot-lib.sh"

"$ROOT/scripts/build-sysroot.sh"

mkdir -p "$ROOT/target"

for triple in "${MYOS_USER_TRIPLES[@]}"; do
  target_dir="$ROOT/target/std-hello-build-${triple}"
  echo "==> std-hello ($triple)"
  myos_cargo_build_app "$triple" release "$target_dir"
  cp "$target_dir/${triple}/release/std-hello" \
    "$ROOT/target/std-hello-${triple}"
  echo "std-hello -> $ROOT/target/std-hello-${triple}"
done
