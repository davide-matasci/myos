#!/usr/bin/env bash
# Obtain a myos sysroot: use an existing install, GHCR (ci-registry), a local
# tarball, or build locally. GHCR is the branch-independent source of truth by
# content hash; the path-filter sysroot job / workflow artifact is optional.
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$HERE/../.." && pwd)"
# shellcheck source=toolchain/std/lib.sh
source "$ROOT/toolchain/std/lib.sh"

if myos_sysroot_is_current; then
  echo "myos sysroot already installed at $MYOS_SYSROOT"
  exit 0
fi

# Prefer GHCR by myos_sysroot_version_hash (scripts/ci-registry.sh port sysroot).
if [[ -x "$ROOT/scripts/ci-registry.sh" ]]; then
  "$ROOT/scripts/ci-registry.sh" pull sysroot || true
  if myos_sysroot_is_current; then
    exit 0
  fi
fi

if [[ -n "${MYOS_SYSROOT_TARBALL:-}" ]]; then
  "$ROOT/toolchain/std/install-sysroot.sh" "$MYOS_SYSROOT_TARBALL"
  exit 0
fi

# Optional: CI artifact path or local package matching current version hash.
want="$(myos_sysroot_version_hash)"
for candidate in \
  "$ROOT/target/myos-sysroot-${want}.tar.zst" \
  "$ROOT/target/myos-sysroot-${want}.tar.gz" \
  "$ROOT/target/myos-sysroot-"*.tar.zst \
  "$ROOT/target/myos-sysroot-"*.tar.gz; do
  [[ -f "$candidate" ]] || continue
  echo "Found local tarball: $candidate"
  "$ROOT/toolchain/std/install-sysroot.sh" "$candidate"
  if myos_sysroot_is_current; then
    exit 0
  fi
  echo "warning: $candidate did not match wanted version $want" >&2
done

echo "No prebuilt sysroot found; building locally..."
"$ROOT/toolchain/std/build-sysroot.sh"

# Publish slim sysroot when CI enables registry push (inherits GITHUB_TOKEN /
# MYOS_CI_REGISTRY_PUSH from the build job). No-op locally / when disabled.
if [[ -x "$ROOT/scripts/ci-registry.sh" ]]; then
  "$ROOT/scripts/ci-registry.sh" push sysroot || true
fi
