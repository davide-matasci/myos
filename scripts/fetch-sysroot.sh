#!/usr/bin/env bash
# Obtain a myos sysroot: use an existing install, a local tarball, or build locally.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# shellcheck source=scripts/myos-sysroot-lib.sh
source "$ROOT/scripts/myos-sysroot-lib.sh"

if myos_sysroot_is_current; then
  echo "myos sysroot already installed at $MYOS_SYSROOT"
  exit 0
fi

if [[ -n "${MYOS_SYSROOT_TARBALL:-}" ]]; then
  "$ROOT/scripts/install-sysroot.sh" "$MYOS_SYSROOT_TARBALL"
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
  "$ROOT/scripts/install-sysroot.sh" "$candidate"
  if myos_sysroot_is_current; then
    exit 0
  fi
  echo "warning: $candidate did not match wanted version $want" >&2
done

echo "No prebuilt sysroot found; building locally..."
"$ROOT/scripts/build-sysroot.sh"
