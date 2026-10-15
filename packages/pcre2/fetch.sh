#!/usr/bin/env bash
# Fetch pinned PCRE2 sources into target/pcre2-src.
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$HERE/../.." && pwd)"
# shellcheck source=versions.env
source "$HERE/versions.env"

SRC="$ROOT/target/pcre2-src"
STAMP="$ROOT/target/.pcre2-src-version"

if [[ -f "$STAMP" && "$(cat "$STAMP")" == "$PCRE2_VERSION" && -f "$SRC/src/pcre2_compile.c" ]]; then
  echo "pcre2 sources up to date ($PCRE2_VERSION)"
  exit 0
fi

rm -rf "$SRC"
mkdir -p "$ROOT/target"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
echo "fetch pcre2 $PCRE2_VERSION"
curl -fsSL --retry 5 --retry-delay 2 "$PCRE2_URL" -o "$tmp/pcre2.tar.bz2"
echo "${PCRE2_SHA256}  $tmp/pcre2.tar.bz2" | sha256sum -c -
mkdir -p "$tmp/extract"
tar -xjf "$tmp/pcre2.tar.bz2" -C "$tmp/extract"
mv "$tmp/extract/pcre2-$PCRE2_VERSION" "$SRC"
echo "$PCRE2_VERSION" >"$STAMP"
echo "pcre2 fetch ok ($PCRE2_VERSION)"
