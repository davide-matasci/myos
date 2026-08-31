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

for triple in "${MYOS_USER_TRIPLES[@]}"; do
  build_coreutils "$triple"
done
