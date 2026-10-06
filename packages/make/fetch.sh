#!/usr/bin/env bash
# Fetch the pinned official GNU make tarball for the myos port.
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$HERE/../.." && pwd)"
# shellcheck source=ports/make/versions.env
source "$HERE/versions.env"

TARBALL="$ROOT/target/make-${MAKE_VERSION}.tar.gz"

# The tarball from the first of MAKE_URLS that answers.
fetch() {
  local url
  mkdir -p "$ROOT/target"
  for url in $MAKE_URLS; do
    echo "==> fetch make $MAKE_VERSION ($url)"
    if curl -fsSL --connect-timeout 20 --retry 2 -o "$TARBALL" "$url"; then
      return
    fi
    echo "fetch $url failed; trying the next mirror" >&2
    rm -f "$TARBALL"
  done
  echo "error: cannot fetch make $MAKE_VERSION" >&2
  exit 1
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
