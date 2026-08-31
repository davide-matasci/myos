#!/usr/bin/env bash
# Build uutils coreutils multicall for myos CI.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# shellcheck source=scripts/myos-sysroot-lib.sh
source "$ROOT/scripts/myos-sysroot-lib.sh"

"$ROOT/scripts/fetch-sysroot.sh"
mkdir -p "$ROOT/target"

build_coreutils() {
  local triple="$1"
  echo "==> uutils coreutils ($triple)"
  MYOS_TARGET="$triple" "$ROOT/scripts/build-coreutils-myos.sh" --release
  local bin="$ROOT/user/uutils-coreutils/target/${triple}/release/coreutils"
  cp "$bin" "$ROOT/target/uutils-coreutils-${triple}"
  cp "$bin" "$ROOT/target/uutils-echo-${triple}"
  cp "$bin" "$ROOT/target/uutils-true-${triple}"
  cp "$bin" "$ROOT/target/uutils-false-${triple}"
  echo "uutils coreutils -> target/uutils-{echo,true,false}-${triple} ($(du -h "$bin" | awk '{print $1}'))"
}

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
  cp "$ROOT/target/uutils-echo-${triple}" "$ROOT/target/uutils-coreutils-${triple}"
}

if [[ "${UUTILS_SMOKE:-0}" == "1" ]]; then
  for triple in "${MYOS_USER_TRIPLES[@]}"; do
    build_smoke "$triple"
  done
else
  for triple in "${MYOS_USER_TRIPLES[@]}"; do
    # Full multicall boots on AArch64; x86 large post-fork exec still faults.
    if [[ "$triple" == "x86_64-unknown-myos" ]]; then
      build_smoke "$triple"
    else
      build_coreutils "$triple"
    fi
  done
fi
