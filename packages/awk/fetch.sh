#!/bin/sh
# Fetch the pinned release of the one true awk.
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
ROOT="$(cd "$HERE/../.." && pwd)"
# shellcheck source=packages/awk/versions.env
. "$HERE/versions.env"

TARBALL="$ROOT/target/awk-${AWK_VERSION}.tar.gz"

# The tarball from the first of AWK_URLS that answers.
fetch() (
  mkdir -p "$ROOT/target"
  for url in $AWK_URLS; do
    echo "==> fetch awk $AWK_VERSION ($url)"
    if curl -fsSL --connect-timeout 20 --retry 2 --retry-all-errors --retry-delay 2 -o "$TARBALL" "$url"; then
      return
    fi
    echo "fetch $url failed; trying the next mirror" >&2
    rm -f "$TARBALL"
  done
  echo "error: cannot fetch awk $AWK_VERSION" >&2
  exit 1
)

if [ -f "$TARBALL" ]; then
  got="$(sha256sum "$TARBALL" | cut -d' ' -f1)"
  if [ "$got" != "$AWK_SHA256" ]; then
    echo "awk tarball checksum mismatch; refetching" >&2
    rm -f "$TARBALL"
    fetch
  fi
else
  fetch
fi

got="$(sha256sum "$TARBALL" | cut -d' ' -f1)"
if [ "$got" != "$AWK_SHA256" ]; then
  echo "awk tarball sha256 mismatch: $got != $AWK_SHA256" >&2
  exit 1
fi
echo "awk tarball ok ($got)"
