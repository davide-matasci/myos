#!/usr/bin/env bash
# Fetch the pinned libedit sources into target/libedit-src.
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$HERE/../.." && pwd)"
# shellcheck source=versions.env
source "$HERE/versions.env"

SRC="$ROOT/target/libedit-src"
STAMP="$ROOT/target/.libedit-src-version"

if [[ -f "$STAMP" && "$(cat "$STAMP")" == "$LIBEDIT_VERSION" && -f "$SRC/src/el.c" ]]; then
  echo "libedit sources up to date ($LIBEDIT_VERSION)"
  exit 0
fi

rm -rf "$SRC"
mkdir -p "$ROOT/target"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
echo "fetch libedit $LIBEDIT_VERSION"
curl -fsSL --retry 5 --retry-delay 2 "$LIBEDIT_URL" -o "$tmp/libedit.tar.gz"
echo "${LIBEDIT_SHA256}  $tmp/libedit.tar.gz" | sha256sum -c -
mkdir -p "$tmp/extract"
tar -xzf "$tmp/libedit.tar.gz" -C "$tmp/extract"
mv "$tmp/extract/libedit-$LIBEDIT_VERSION" "$SRC"
echo "$LIBEDIT_VERSION" >"$STAMP"
echo "libedit fetch ok ($LIBEDIT_VERSION)"
