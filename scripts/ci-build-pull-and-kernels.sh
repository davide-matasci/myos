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
./scripts/ci-registry.sh pull newlib || true
./scripts/ci-registry.sh pull std-hello || true
./scripts/ci-registry.sh pull c-hello || true
./scripts/ci-registry.sh pull curl || true
./scripts/ci-registry.sh pull dropbear || true
./scripts/ci-registry.sh pull sbase || true
./scripts/ci-registry.sh pull oksh || true
./scripts/ci-registry.sh pull make || true
./scripts/ci-registry.sh pull ubase || true
./scripts/ci-registry.sh pull coreutils || true
./scripts/ci-registry.sh pull ripgrep || true
./scripts/ci-registry.sh pull tcc || true
./scripts/ci-registry.sh pull ncurses || true
./scripts/ci-registry.sh pull vim || true
./scripts/ci-registry.sh pull zlib || true
./scripts/ci-registry.sh pull git || true
./scripts/ci-registry.sh pull lynx || true
./scripts/ci-registry.sh pull lua || true
./scripts/ci-registry.sh push sysroot || true
./scripts/ci-registry.sh push newlib || true
./scripts/ci-registry.sh push std-hello || true
./scripts/ci-registry.sh push c-hello || true
./scripts/ci-registry.sh push curl || true
./scripts/ci-registry.sh push dropbear || true
./scripts/ci-registry.sh push sbase || true
./scripts/ci-registry.sh push oksh || true
./scripts/ci-registry.sh push make || true
./scripts/ci-registry.sh push ubase || true
./scripts/ci-registry.sh push coreutils || true
./scripts/ci-registry.sh push ripgrep || true
./scripts/ci-registry.sh push tcc || true
./scripts/ci-registry.sh push ncurses || true
./scripts/ci-registry.sh push vim || true
./scripts/ci-registry.sh push zlib || true
./scripts/ci-registry.sh push git || true
./scripts/ci-registry.sh push lynx || true
./scripts/ci-registry.sh push lua || true
./scripts/ci-build-kernels.sh

