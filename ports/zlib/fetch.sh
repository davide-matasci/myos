#!/usr/bin/env bash
# Fetch pinned zlib sources into target/.
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$HERE/../.." && pwd)"
# shellcheck source=versions.env
source "$HERE/versions.env"

SRC="$ROOT/target/zlib-src"
STAMP="$ROOT/target/.zlib-src-version"
WANT="${ZLIB_VERSION}"

if [[ -f "$STAMP" && "$(cat "$STAMP")" == "$WANT" && -f "$SRC/zlib.h" && -f "$SRC/deflate.c" ]]; then
  echo "zlib sources up to date ($WANT)"
  exit 0
fi

rm -rf "$SRC"
mkdir -p "$ROOT/target"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
echo "fetch zlib $WANT"
curl -fsSL "$ZLIB_URL" -o "$tmp/zlib.tgz"
if command -v sha256sum >/dev/null && [[ -n "${ZLIB_SHA256:-}" ]]; then
  echo "${ZLIB_SHA256}  $tmp/zlib.tgz" | sha256sum -c -
fi
mkdir -p "$tmp/extract"
tar -xzf "$tmp/zlib.tgz" -C "$tmp/extract"
mv "$tmp/extract"/zlib-* "$SRC"
echo "$WANT" >"$STAMP"
echo "zlib fetch ok ($WANT / $ZLIB_REV)"
