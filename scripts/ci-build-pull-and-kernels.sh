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
# Every port with a build script in build order (scripts/ports.sh
# --build-list all: newlib first, the sysroot was fetched above; the image
# ports and the packages, which the build job packs too) and the Linux
# layer's musl pieces. A miss is built by ci-build-kernels.sh and pushed
# from there.
pieces=()
while read -r name _; do
  [[ "$name" == sysroot ]] || pieces+=("$name")
done < <(./scripts/ports.sh --build-list all)
pieces+=(linux-compat)
for p in "${pieces[@]}"; do
  ./scripts/ci-registry.sh pull "$p" || true
done
for p in "${pieces[@]}"; do
  ./scripts/ci-registry.sh push "$p" || true
done
./scripts/ci-build-kernels.sh
