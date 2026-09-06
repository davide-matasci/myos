#!/usr/bin/env bash
# Fetch curl sources into target/curl-src.
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$HERE/../.." && pwd)"
# shellcheck source=versions.env
source "$HERE/versions.env"

SRC="$ROOT/target/curl-src"
TGZ="$ROOT/target/curl-${CURL_VERSION}.tar.gz"
MARKER="$SRC/.myos-curl-version"
TAG="curl-${CURL_VERSION//./_}"

if [[ -f "$MARKER" && "$(cat "$MARKER")" == "$CURL_VERSION" && -f "$SRC/include/curl/curl.h" ]]; then
  echo "curl source up to date ($CURL_VERSION)"
  exit 0
fi

mkdir -p "$ROOT/target"
if [[ ! -f "$TGZ" ]]; then
  echo "fetching curl $CURL_VERSION"
  curl -fsSL -L -o "$TGZ" "https://github.com/curl/curl/releases/download/${TAG}/curl-${CURL_VERSION}.tar.gz"
fi

rm -rf "$SRC"
tar -xzf "$TGZ" -C "$ROOT/target"
if [[ -d "$ROOT/target/curl-${CURL_VERSION}" ]]; then
  mv "$ROOT/target/curl-${CURL_VERSION}" "$SRC"
else
  found="$(find "$ROOT/target" -maxdepth 1 -type d -name 'curl-*' ! -name 'curl-src' | head -1)"
  [[ -n "$found" ]] || { echo "curl extract failed"; exit 1; }
  mv "$found" "$SRC"
fi
echo "$CURL_VERSION" >"$MARKER"
echo "curl source ready at $SRC"
