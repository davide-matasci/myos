#!/usr/bin/env bash
# Fetch mbedtls sources + Mozilla CA bundle into target/.
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$HERE/../.." && pwd)"
# shellcheck source=versions.env
source "$HERE/versions.env"

SRC="$ROOT/target/mbedtls-src"
STAMP="$ROOT/target/.mbedtls-src-version"
WANT="${MBEDTLS_VERSION}"

if [[ -f "$STAMP" && "$(cat "$STAMP")" == "$WANT" && -d "$SRC/library" ]]; then
  echo "mbedtls sources up to date ($WANT)"
else
  rm -rf "$SRC"
  mkdir -p "$ROOT/target"
  tmp="$(mktemp -d)"
  trap 'rm -rf "$tmp"' EXIT
  echo "fetch mbedtls $WANT"
  curl -fsSL "$MBEDTLS_URL" -o "$tmp/mbedtls.tgz"
  if command -v sha256sum >/dev/null && [[ -n "${MBEDTLS_SHA256:-}" ]]; then
    echo "${MBEDTLS_SHA256}  $tmp/mbedtls.tgz" | sha256sum -c -
  fi
  mkdir -p "$tmp/extract"
  tar -xzf "$tmp/mbedtls.tgz" -C "$tmp/extract"
  mv "$tmp/extract"/mbedtls-* "$SRC"
  rm -rf "$SRC/programs" "$SRC/tests" "$SRC/docs"
  echo "$WANT" >"$STAMP"
fi

CA="$ROOT/target/cacert.pem"
if [[ ! -f "$CA" ]]; then
  echo "fetch CA bundle"
  curl -fsSL "$CACERT_URL" -o "$CA"
fi
echo "mbedtls fetch ok"
