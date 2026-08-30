#!/usr/bin/env bash
# Build uutils smoke ELFs for myos CI (and optionally full coreutils for dev).
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# shellcheck source=scripts/myos-sysroot-lib.sh
source "$ROOT/scripts/myos-sysroot-lib.sh"

"$ROOT/scripts/fetch-sysroot.sh"
mkdir -p "$ROOT/target"

build_smoke() {
  local triple="$1"
  local target_dir="$ROOT/target/uutils-smoke-build-${triple}"
  echo "==> uutils smoke ($triple)"
  myos_cargo_build_app "$triple" release "$target_dir" \
    "$ROOT/std/examples/uutils-smoke/Cargo.toml" uutils-echo-smoke
  myos_cargo_build_app "$triple" release "$target_dir" \
    "$ROOT/std/examples/uutils-smoke/Cargo.toml" uutils-true-smoke
  myos_cargo_build_app "$triple" release "$target_dir" \
    "$ROOT/std/examples/uutils-smoke/Cargo.toml" uutils-false-smoke
  cp "$target_dir/${triple}/release/uutils-echo-smoke" \
    "$ROOT/target/uutils-echo-${triple}"
  cp "$target_dir/${triple}/release/uutils-true-smoke" \
    "$ROOT/target/uutils-true-${triple}"
  cp "$target_dir/${triple}/release/uutils-false-smoke" \
    "$ROOT/target/uutils-false-${triple}"
  # Legacy path: kernel embed reads one env var; keep echo ELF for tooling.
  cp "$ROOT/target/uutils-echo-${triple}" "$ROOT/target/uutils-coreutils-${triple}"
  echo "uutils smoke -> target/uutils-{echo,true,false}-${triple}"
}

for triple in "${MYOS_USER_TRIPLES[@]}"; do
  build_smoke "$triple"
done

if [[ "${UUTILS_FULL:-0}" == "1" ]]; then
  echo "==> also building full uutils coreutils (UUTILS_FULL=1)"
  for triple in "${MYOS_USER_TRIPLES[@]}"; do
    MYOS_TARGET="$triple" "$ROOT/scripts/build-coreutils-myos.sh" --release
    cp "$ROOT/user/uutils-coreutils/target/${triple}/release/coreutils" \
      "$ROOT/target/uutils-coreutils-full-${triple}"
  done
fi
