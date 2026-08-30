#!/usr/bin/env bash
# Build uutils coreutils multicall for myos and install stable CI/kernel paths.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# shellcheck source=scripts/myos-sysroot-lib.sh
source "$ROOT/scripts/myos-sysroot-lib.sh"

"$ROOT/scripts/fetch-sysroot.sh"
mkdir -p "$ROOT/target"

for triple in "${MYOS_USER_TRIPLES[@]}"; do
  echo "==> uutils coreutils ($triple)"
  MYOS_TARGET="$triple" "$ROOT/scripts/build-coreutils-myos.sh" --release
  src="$ROOT/user/uutils-coreutils/target/${triple}/release/coreutils"
  dest="$ROOT/target/uutils-coreutils-${triple}"
  cp "$src" "$dest"
  echo "uutils-coreutils -> $dest ($(du -h "$dest" | awk '{print $1}'))"
done
