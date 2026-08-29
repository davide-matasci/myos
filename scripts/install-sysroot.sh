#!/usr/bin/env bash
# Install a packaged myos sysroot tarball into target/myos-sysroot.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# shellcheck source=scripts/myos-sysroot-lib.sh
source "$ROOT/scripts/myos-sysroot-lib.sh"

TARBALL="${1:?usage: install-sysroot.sh <myos-sysroot-*.tar.zst|*.tar.gz>}"

if [[ ! -f "$TARBALL" ]]; then
  echo "error: tarball not found: $TARBALL" >&2
  exit 1
fi

mkdir -p "$(dirname "$MYOS_SYSROOT")"
rm -rf "$MYOS_SYSROOT"
tmpdir="$(mktemp -d "${TMPDIR:-/tmp}/myos-sysroot-unpack.XXXXXX")"
trap 'rm -rf "$tmpdir"' EXIT

echo "Extracting $TARBALL -> $MYOS_SYSROOT"
case "$TARBALL" in
  *.tar.zst)
    if ! command -v zstd >/dev/null 2>&1; then
      echo "error: zstd required to extract .tar.zst" >&2
      exit 1
    fi
    zstd -d -c "$TARBALL" | tar -C "$tmpdir" -xf -
    ;;
  *.tar.gz|*.tgz)
    tar -C "$tmpdir" -xzf "$TARBALL"
    ;;
  *)
    echo "error: unsupported tarball format (want .tar.zst or .tar.gz)" >&2
    exit 1
    ;;
esac

if [[ ! -d "$tmpdir/myos-sysroot" ]]; then
  echo "error: tarball missing top-level myos-sysroot/ directory" >&2
  exit 1
fi
mv "$tmpdir/myos-sysroot" "$MYOS_SYSROOT"

if [[ ! -f "$MYOS_SYSROOT/myos-manifest.toml" ]]; then
  echo "error: extracted tree missing myos-manifest.toml" >&2
  exit 1
fi

echo "installed sysroot $(cat "$MYOS_SYSROOT/.myos-sysroot-version" 2>/dev/null || echo '?')"
cat "$MYOS_SYSROOT/myos-manifest.toml"
