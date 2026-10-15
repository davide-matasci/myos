#!/usr/bin/env bash
# Fetch pinned nginx sources into target/nginx-src.
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$HERE/../.." && pwd)"
# shellcheck source=versions.env
source "$HERE/versions.env"

SRC="$ROOT/target/nginx-src"
STAMP="$ROOT/target/.nginx-src-version"

if [[ -f "$STAMP" && "$(cat "$STAMP")" == "$NGINX_VERSION" && -f "$SRC/src/core/nginx.c" ]]; then
  echo "nginx sources up to date ($NGINX_VERSION)"
  exit 0
fi

rm -rf "$SRC"
mkdir -p "$ROOT/target"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
echo "fetch nginx $NGINX_VERSION"
curl -fsSL --retry 5 --retry-delay 2 "$NGINX_URL" -o "$tmp/nginx.tar.gz"
echo "${NGINX_SHA256}  $tmp/nginx.tar.gz" | sha256sum -c -
mkdir -p "$tmp/extract"
tar -xzf "$tmp/nginx.tar.gz" -C "$tmp/extract"
mv "$tmp/extract/nginx-$NGINX_VERSION" "$SRC"
echo "$NGINX_VERSION" >"$STAMP"
echo "nginx fetch ok ($NGINX_VERSION)"
