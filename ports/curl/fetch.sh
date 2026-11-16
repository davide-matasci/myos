#!/bin/sh
# Fetch the curl sources into target/curl-src, and the Mozilla CA bundle.
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
ROOT="$(cd "$HERE/../.." && pwd)"
# shellcheck source=versions.env
. "$HERE/versions.env"

SRC="$ROOT/target/curl-src"
TGZ="$ROOT/target/curl-${CURL_VERSION}.tar.gz"
MARKER="$SRC/.myos-curl-version"
TAG="curl-$(printf '%s\n' "$CURL_VERSION" | tr . _)"

# The CA bundle (an output of the port, /lib/cacert.pem in the image): once
# per tree, the latest.
fetch_ca_bundle() (
  ca="$ROOT/target/cacert.pem"
  if [ ! -f "$ca" ]; then
    echo "fetch CA bundle"
    curl -fsSL "$CACERT_URL" -o "$ca"
  fi
)

mkdir -p "$ROOT/target"
fetch_ca_bundle
if [ -f "$MARKER" ] && [ "$(cat "$MARKER")" = "$CURL_VERSION" ] && [ -f "$SRC/include/curl/curl.h" ]; then
  echo "curl source up to date ($CURL_VERSION)"
  exit 0
fi

if [ ! -f "$TGZ" ]; then
  echo "fetching curl $CURL_VERSION"
  curl -fsSL -L -o "$TGZ" "https://github.com/curl/curl/releases/download/${TAG}/curl-${CURL_VERSION}.tar.gz"
fi

rm -rf "$SRC"
tar -xzf "$TGZ" -C "$ROOT/target"
if [ -d "$ROOT/target/curl-${CURL_VERSION}" ]; then
  mv "$ROOT/target/curl-${CURL_VERSION}" "$SRC"
else
  found="$(find "$ROOT/target" -maxdepth 1 -type d -name 'curl-*' ! -name 'curl-src' | head -1)"
  [ -n "$found" ] || { echo "curl extract failed"; exit 1; }
  mv "$found" "$SRC"
fi
echo "$CURL_VERSION" >"$MARKER"
echo "curl source ready at $SRC"
