#!/usr/bin/env bash
# Fetch the pinned official Lua tarball for the myos port.
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$HERE/../.." && pwd)"
# shellcheck source=ports/lua/versions.env
source "$HERE/versions.env"

TARBALL="$ROOT/target/lua-${LUA_VERSION}.tar.gz"
URL="$LUA_URL"

fetch() {
  echo "==> fetch lua $LUA_VERSION ($URL)"
  mkdir -p "$ROOT/target"
  if command -v curl >/dev/null 2>&1; then
    curl -fsSL -o "$TARBALL" "$URL"
  else
    wget -q -O "$TARBALL" "$URL"
  fi
}

if [[ -f "$TARBALL" ]]; then
  got="$(sha256sum "$TARBALL" | cut -d' ' -f1)"
  if [[ "$got" != "$LUA_SHA256" ]]; then
    echo "lua tarball checksum mismatch; refetching" >&2
    rm -f "$TARBALL"
    fetch
  fi
else
  fetch
fi

got="$(sha256sum "$TARBALL" | cut -d' ' -f1)"
if [[ "$got" != "$LUA_SHA256" ]]; then
  echo "lua tarball sha256 mismatch: $got != $LUA_SHA256" >&2
  exit 1
fi
echo "lua tarball ok ($got)"
