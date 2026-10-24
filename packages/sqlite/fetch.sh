#!/usr/bin/env bash
# Fetch the pinned SQLite amalgamation into target/sqlite-src.
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$HERE/../.." && pwd)"
# shellcheck source=versions.env
source "$HERE/versions.env"

SRC="$ROOT/target/sqlite-src"
STAMP="$ROOT/target/.sqlite-src-version"

if [[ -f "$STAMP" && "$(cat "$STAMP")" == "$SQLITE_VERSION" && -f "$SRC/sqlite3.c" ]]; then
  echo "sqlite sources up to date ($SQLITE_VERSION)"
  exit 0
fi

rm -rf "$SRC"
mkdir -p "$ROOT/target"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
echo "fetch sqlite $SQLITE_VERSION"
curl -fsSL --retry 5 --retry-delay 2 "$SQLITE_URL" -o "$tmp/sqlite.tar.gz"
echo "${SQLITE_SHA256}  $tmp/sqlite.tar.gz" | sha256sum -c -
mkdir -p "$tmp/extract"
tar -xzf "$tmp/sqlite.tar.gz" -C "$tmp/extract"
mv "$tmp/extract/${SQLITE_TARBALL%.tar.gz}" "$SRC"
echo "$SQLITE_VERSION" >"$STAMP"
echo "sqlite fetch ok ($SQLITE_VERSION)"
