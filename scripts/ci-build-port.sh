#!/usr/bin/env bash
# CI helper for the ports job (not boot): pull a port's dependencies (and
# build the ones the registry does not have), the optional sysroot, build
# the port, push it to GHCR.
# Args: PORT SCRIPT [NEEDS_SYSROOT] [DEPS]
# PORT names the registry package (a port's name); DEPS is the space-separated
# PORT_DEPS of its descriptor.
set -euo pipefail
chmod +x scripts/*.sh ports/*/*.sh toolchain/*/*.sh 2>/dev/null || true

PORT="${1:?port name}"
SCRIPT="${2:?build script}"
NEEDS_SYSROOT="${3:-0}"
DEPS="${4:-}"

./scripts/ci-registry.sh pull newlib || true
for dep in $DEPS; do
  ./scripts/ci-registry.sh pull "$dep" || true
  if ! ./scripts/ci-registry.sh current "$dep"; then
    # Built here too (every port needing it does the same on a miss; the
    # registry keeps the first push), so the matrix has no second stage.
    echo "==> dependency $dep not in the registry; building it"
    "./$(./scripts/ports.sh --script "$dep")"
    ./scripts/ci-registry.sh push "$dep" || true
  fi
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
