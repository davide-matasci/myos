#!/usr/bin/env bash
# Pull GHCR ports (best-effort) and build kernels for the CI build job.
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"
chmod +x scripts/*.sh ports/*/*.sh toolchain/*/*.sh
./scripts/ci-registry.sh pull sysroot || true
if compgen -G "target/myos-sysroot-*.tar.zst" > /dev/null; then
  export MYOS_SYSROOT_TARBALL="$(ls target/myos-sysroot-*.tar.zst | head -1)"
  ./toolchain/std/fetch-sysroot.sh
else
  ./toolchain/std/fetch-sysroot.sh
fi
# Everything the images carry: newlib, every image port with a build script
# (scripts/ports.sh --image-list) and the Linux layer's musl pieces. A miss
# is built by ci-build-kernels.sh and pushed from there.
pieces=(newlib)
while read -r name _; do
  pieces+=("$name")
done < <(./scripts/ports.sh --image-list)
pieces+=(linux-compat)
for p in "${pieces[@]}"; do
  ./scripts/ci-registry.sh pull "$p" || true
done
for p in "${pieces[@]}"; do
  ./scripts/ci-registry.sh push "$p" || true
done
./scripts/ci-build-kernels.sh
