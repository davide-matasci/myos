#!/usr/bin/env bash
# Fetch the pinned official GNU make tarball for the myos port.
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$HERE/../.." && pwd)"
# shellcheck source=ports/make/versions.env
source "$HERE/versions.env"

TARBALL="$ROOT/target/make-${MAKE_VERSION}.tar.gz"
URL="https://ftp.gnu.org/gnu/make/make-${MAKE_VERSION}.tar.gz"

fetch() {
  echo "==> fetch make $MAKE_VERSION ($URL)"
  mkdir -p "$ROOT/target"
  if command -v curl >/dev/null 2>&1; then
    curl -fsSL -o "$TARBALL" "$URL"
  else
    wget -q -O "$TARBALL" "$URL"
  fi
}

if [[ -f "$TARBALL" ]]; then
  got="$(sha256sum "$TARBALL" | cut -d' ' -f1)"
  if [[ "$got" != "$MAKE_SHA256" ]]; then
    echo "make tarball checksum mismatch; refetching" >&2
    rm -f "$TARBALL"
    fetch
  fi
else
  fetch
fi

got="$(sha256sum "$TARBALL" | cut -d' ' -f1)"
if [[ "$got" != "$MAKE_SHA256" ]]; then
  echo "make tarball sha256 mismatch: $got != $MAKE_SHA256" >&2
  exit 1
fi
echo "make tarball ok ($got)"
