#!/usr/bin/env bash
# CI helper for the ports job (not boot): pull a port's dependencies (and
# build the ones the registry does not have), the optional sysroot, build
# the port, push it to GHCR.
# Args: PORT SCRIPT [NEEDS_SYSROOT] [DEPS]
# PORT names the registry package (a port's name, or `<name>:<arch>` for an
# arch of a port built an arch at a time: the script builds that arch,
# MYOS_<NAME>_ARCHES); DEPS is the space-separated PORT_DEPS of its
# descriptor, what a piece without a descriptor (linux-compat) depends on:
# a port's are all the ports it builds against, transitively, in build
# order (scripts/ports.sh --deps).
set -euo pipefail
chmod +x scripts/*.sh ports/*/*.sh toolchain/*/*.sh 2>/dev/null || true

PORT="${1:?port name}"
SCRIPT="${2:?build script}"
NEEDS_SYSROOT="${3:-0}"
DEPS="${4:-}"
if closure="$(./scripts/ports.sh --deps "${PORT%%:*}" 2>/dev/null)"; then
  DEPS="$closure"
fi

./scripts/ci-registry.sh pull newlib || true
# Each dependency in build order, so that it is keyed on the registry's
# outputs of the ones before it, as every other job keys it.
for dep in $DEPS; do
  ./scripts/ci-registry.sh pull "$dep" || true
  if ! ./scripts/ci-registry.sh current "$dep"; then
    # Built here too (every port needing it does the same on a miss; the
    # registry keeps the first push), so the matrix has no second stage.
    echo "==> dependency $dep not in the registry; building it"
    "./$(./scripts/ports.sh --script "$dep")"
    # Pushed; or another job pushed it first, and that build replaces this
    # one here: a build is not always the same bytes twice (docs/ports.md
    # "Cache keys"), and what is keyed on it must be the registry's.
    if ./scripts/ci-registry.sh push "$dep" | tee /dev/stderr | grep -q 'already present'; then
      echo "==> dependency $dep pushed by another job first; taking its build"
      ./scripts/ci-registry.sh pull "$dep" || true
    fi
  fi
done
./scripts/ci-registry.sh pull "$PORT" || true
if [[ "$NEEDS_SYSROOT" == "1" ]]; then
  if compgen -G "target/myos-sysroot-*.tar.zst" > /dev/null; then
    export MYOS_SYSROOT_TARBALL="$(ls target/myos-sysroot-*.tar.zst | head -1)"
  fi
  ./toolchain/std/fetch-sysroot.sh
fi
if [[ "$PORT" == *:* ]]; then
  name="$(printf '%s' "${PORT%%:*}" | tr '[:lower:]-' '[:upper:]_')"
  export "MYOS_${name}_ARCHES=${PORT#*:}"
fi
# matrix.script is a shell command string (path or path+args)
bash -c "$SCRIPT"
./scripts/ci-registry.sh push "$PORT" || true
