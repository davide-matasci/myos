#!/usr/bin/env bash
# Build a versioned myos sysroot with precompiled std for x86_64 and AArch64.
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$HERE/../.." && pwd)"
# shellcheck source=toolchain/std/lib.sh
source "$ROOT/toolchain/std/lib.sh"

if myos_sysroot_is_current; then
  echo "myos sysroot up to date at $MYOS_SYSROOT"
  exit 0
fi

echo "Building myos sysroot (toolchain $MYOS_NIGHTLY)..."
"$ROOT/toolchain/std/prepare.sh"

for triple in "${MYOS_USER_TRIPLES[@]}"; do
  echo "==> prebuilding std for $triple"
  myos_cargo_build_std "$triple" release
done

version="$(myos_sysroot_version_hash)"
echo "$version" >"$MYOS_SYSROOT_VERSION"
myos_write_sysroot_manifest "$version"
echo "myos sysroot ready at $MYOS_SYSROOT"

# Touch for CI: ensure boot jobs run when sysroot path filter fires (skip-propagation).
