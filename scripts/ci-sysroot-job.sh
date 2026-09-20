#!/usr/bin/env bash
# CI sysroot job body (build/package + optional GHCR). Not used by boot.
set -euo pipefail
chmod +x scripts/*.sh ports/*/*.sh toolchain/*/*.sh 2>/dev/null || true
if [[ "${MYOS_FORCE_SYSROOT:-}" == "1" ]]; then
  rm -f target/myos-sysroot/.myos-sysroot-version
fi
./scripts/ci-registry.sh pull sysroot || true
source ./toolchain/std/lib.sh
if myos_sysroot_is_current; then
  echo "sysroot up to date"
else
  ./toolchain/std/build-sysroot.sh
  ./scripts/ci-registry.sh push sysroot || true
fi
./toolchain/std/package-sysroot.sh
