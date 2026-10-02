#!/usr/bin/env bash
# CI helper: pull deps, optional sysroot, build one port, push GHCR.
# Args: PORT SCRIPT [NEEDS_SYSROOT] [DEPS]
# PORT names the registry package (a port's name); DEPS is the space-separated
# PORT_DEPS of its descriptor. Used by ports-base / ports-advanced (not boot).
set -euo pipefail
chmod +x scripts/*.sh ports/*/*.sh toolchain/*/*.sh 2>/dev/null || true

PORT="${1:?port name}"
SCRIPT="${2:?build script}"
NEEDS_SYSROOT="${3:-0}"
DEPS="${4:-}"

./scripts/ci-registry.sh pull newlib || true
for dep in $DEPS; do
  ./scripts/ci-registry.sh pull "$dep" || true
done
./scripts/ci-registry.sh pull "$PORT" || true
if [[ "$NEEDS_SYSROOT" == "1" ]]; then
  if compgen -G "target/myos-sysroot-*.tar.zst" > /dev/null; then
    export MYOS_SYSROOT_TARBALL="$(ls target/myos-sysroot-*.tar.zst | head -1)"
  fi
  ./toolchain/std/fetch-sysroot.sh
fi
# matrix.script is a shell command string (path or path+args)
bash -c "$SCRIPT"
./scripts/ci-registry.sh push "$PORT" || true
