#!/usr/bin/env bash
# Fetch the pinned official CPython tarball for the myos package.
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$HERE/../.." && pwd)"
# shellcheck source=packages/python/versions.env
source "$HERE/versions.env"

TARBALL="$ROOT/target/Python-${PYTHON_VERSION}.tar.xz"

fetch() {
  echo "==> fetch python $PYTHON_VERSION ($PYTHON_URL)"
  mkdir -p "$ROOT/target"
  if command -v curl >/dev/null 2>&1; then
    curl -fsSL -o "$TARBALL" "$PYTHON_URL"
  else
    wget -q -O "$TARBALL" "$PYTHON_URL"
  fi
}

if [[ -f "$TARBALL" ]]; then
  got="$(sha256sum "$TARBALL" | cut -d' ' -f1)"
  if [[ "$got" != "$PYTHON_SHA256" ]]; then
    echo "python tarball checksum mismatch; refetching" >&2
    rm -f "$TARBALL"
    fetch
  fi
else
  fetch
fi

got="$(sha256sum "$TARBALL" | cut -d' ' -f1)"
if [[ "$got" != "$PYTHON_SHA256" ]]; then
  echo "python tarball sha256 mismatch: $got != $PYTHON_SHA256" >&2
  exit 1
fi
echo "python tarball ok ($got)"
