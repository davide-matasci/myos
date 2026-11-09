#!/usr/bin/env bash
# Fetch the pinned official Lua tarball for the myos port.
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$HERE/../.." && pwd)"
# shellcheck source=ports/lua/versions.env
source "$HERE/versions.env"

TARBALL="$ROOT/target/lua-${LUA_VERSION}.tar.gz"

# The tarball from the first of LUA_URLS that answers.
fetch() {
  local url
  mkdir -p "$ROOT/target"
  for url in $LUA_URLS; do
    echo "==> fetch lua $LUA_VERSION ($url)"
    if curl -fsSL --connect-timeout 20 --retry 2 --retry-all-errors --retry-delay 2 -o "$TARBALL" "$url"; then
      return
    fi
    echo "fetch $url failed; trying the next mirror" >&2
    rm -f "$TARBALL"
  done
  echo "error: cannot fetch lua $LUA_VERSION" >&2
  exit 1
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
