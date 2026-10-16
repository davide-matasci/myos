#!/usr/bin/env bash
# Fetch the openssl(1) sources into target/openssl-src: apps/openssl, the
# compat headers (what newlib lacks, declared) and the default configuration
# files of the LibreSSL tarball ports/libressl pins (that port's own tree is
# not there when its libraries came from the CI registry).
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$HERE/../.." && pwd)"
# shellcheck source=ports/libressl/versions.env
source "$ROOT/ports/libressl/versions.env"

SRC="$ROOT/target/openssl-src"
STAMP="$ROOT/target/.openssl-src-version"

if [[ -f "$STAMP" && "$(cat "$STAMP")" == "$LIBRESSL_VERSION" && -f "$SRC/apps/openssl/openssl.c" \
    && -f "$SRC/include/compat/stdlib.h" ]]; then
  echo "openssl sources up to date ($LIBRESSL_VERSION)"
  exit 0
fi

rm -rf "$SRC"
mkdir -p "$ROOT/target"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
echo "fetch libressl $LIBRESSL_VERSION (apps/openssl)"
curl -fsSL "$LIBRESSL_URL" -o "$tmp/libressl.tgz"
echo "${LIBRESSL_SHA256}  $tmp/libressl.tgz" | sha256sum -c -
mkdir -p "$tmp/extract"
v="libressl-${LIBRESSL_VERSION}"
tar -xzf "$tmp/libressl.tgz" -C "$tmp/extract" \
  "$v/apps/openssl" "$v/include/compat" "$v/openssl.cnf" "$v/x509v3.cnf" "$v/COPYING"
mv "$tmp/extract/$v" "$SRC"
echo "$LIBRESSL_VERSION" >"$STAMP"
echo "openssl fetch ok"
