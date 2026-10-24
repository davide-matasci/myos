#!/usr/bin/env bash
# Fetch the LibreSSL sources into target/libressl-src, with myos's patches
# (*.myos.patch: the default CA file).
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$HERE/../.." && pwd)"
# shellcheck source=versions.env
source "$HERE/versions.env"

SRC="$ROOT/target/libressl-src"
STAMP="$ROOT/target/.libressl-src-version"
WANT="$LIBRESSL_VERSION $(cat "$HERE"/*.myos.patch | sha256sum | cut -d' ' -f1)"

if [[ -f "$STAMP" && "$(cat "$STAMP")" == "$WANT" && -d "$SRC/crypto" ]]; then
  echo "libressl sources up to date ($LIBRESSL_VERSION)"
  exit 0
fi

rm -rf "$SRC"
mkdir -p "$ROOT/target"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
echo "fetch libressl $LIBRESSL_VERSION"
curl -fsSL "$LIBRESSL_URL" -o "$tmp/libressl.tgz"
echo "${LIBRESSL_SHA256}  $tmp/libressl.tgz" | sha256sum -c -
mkdir -p "$tmp/extract"
tar -xzf "$tmp/libressl.tgz" -C "$tmp/extract"
mv "$tmp/extract/libressl-${LIBRESSL_VERSION}" "$SRC"
# The libraries only: not the openssl(1) and nc(1) programs, the tests and
# the manuals.
rm -rf "$SRC/apps" "$SRC/tests" "$SRC/man"
for p in "$HERE"/*.myos.patch; do
  patch -d "$SRC" -p1 -s <"$p"
done
echo "$WANT" >"$STAMP"
echo "libressl fetch ok"
