#!/usr/bin/env bash
# CI helper: pull deps, optional sysroot, build one port, push GHCR.
# Args: PULL SCRIPT [NEEDS_SYSROOT] [DEP_PORT]
# Used by ports-base / ports-advanced (not boot).
set -euo pipefail
chmod +x scripts/*.sh ports/*/*.sh toolchain/*/*.sh 2>/dev/null || true

PULL="${1:?pull name}"
SCRIPT="${2:?build script}"
NEEDS_SYSROOT="${3:-0}"
DEP_PORT="${4:-}"

./scripts/ci-registry.sh pull newlib || true
if [[ -n "$DEP_PORT" ]]; then
  ./scripts/ci-registry.sh pull "$DEP_PORT" || true
fi
./scripts/ci-registry.sh pull "$PULL" || true
if [[ "$NEEDS_SYSROOT" == "1" ]]; then
  if compgen -G "target/myos-sysroot-*.tar.zst" > /dev/null; then
    export MYOS_SYSROOT_TARBALL="$(ls target/myos-sysroot-*.tar.zst | head -1)"
  fi
  ./toolchain/std/fetch-sysroot.sh
fi
# matrix.script is a shell command string (path or path+args)
bash -c "$SCRIPT"
./scripts/ci-registry.sh push "$PULL" || true
