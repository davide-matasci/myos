#!/usr/bin/env bash
# Package the prebuilt myos sysroot for local reuse or CI workflow artifacts.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# shellcheck source=scripts/myos-sysroot-lib.sh
source "$ROOT/scripts/myos-sysroot-lib.sh"

"$ROOT/scripts/build-sysroot.sh"

version="$(cat "$MYOS_SYSROOT_VERSION")"
zstd_level="${MYOS_ZSTD_LEVEL:-19}"
if command -v zstd >/dev/null 2>&1; then
  out="$ROOT/target/myos-sysroot-${version}.tar.zst"
else
  out="$ROOT/target/myos-sysroot-${version}.tar.gz"
fi

if [[ -f "$out" ]]; then
  echo "packaged sysroot already present -> $out"
  exit 0
fi

if [[ "${MYOS_SYSROOT_SLIM:-}" == "1" ]]; then
  # CI artifact: precompiled rlibs + target specs only (~tens of MB). Skip the
  # patched rust library/ tree (~1.5 GiB) that makes zstd -19 take minutes.
  tmp="$(mktemp -d "${TMPDIR:-/tmp}/myos-sysroot-slim.XXXXXX")"
  trap 'rm -rf "$tmp"' EXIT
  mkdir -p "$tmp/myos-sysroot/lib/rustlib"
  cp "$MYOS_SYSROOT/myos-manifest.toml" "$MYOS_SYSROOT/.myos-sysroot-version" "$tmp/myos-sysroot/"
  for triple in "${MYOS_USER_TRIPLES[@]}"; do
    cp -a "$MYOS_SYSROOT/lib/rustlib/${triple}" "$tmp/myos-sysroot/lib/rustlib/"
    cp "$MYOS_SYSROOT/lib/rustlib/${triple}.json" "$tmp/myos-sysroot/lib/rustlib/"
  done
  if command -v zstd >/dev/null 2>&1; then
    tar -C "$tmp" -cf - myos-sysroot | zstd -T0 "-${zstd_level}" -o "$out"
  else
    tar -C "$tmp" -czf "$out" myos-sysroot
    echo "note: zstd not found; packaged with gzip" >&2
  fi
elif command -v zstd >/dev/null 2>&1; then
  tar -C "$ROOT/target" -cf - myos-sysroot | zstd -T0 "-${zstd_level}" -o "$out"
else
  tar -C "$ROOT/target" -czf "$out" myos-sysroot
  echo "note: zstd not found; packaged with gzip" >&2
fi
echo "packaged sysroot -> $out"
