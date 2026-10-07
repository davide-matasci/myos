#!/usr/bin/env bash
# smoltcp with myos's patches (*.myos.patch here) for netd: the pinned crate
# (versions.env) from crates.io, checked against its SHA-256, unpacked and
# patched into target/smoltcp-myos, which user/netd/Cargo.toml's
# [patch.crates-io] names. kernel/build.rs runs it before it builds netd
# (PORT_PREPARE); nothing to do when target/ already has this version with
# these patches.
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$HERE/../../.." && pwd)"
# shellcheck source=user/net/smoltcp/versions.env
source "$HERE/versions.env"

DEST="$ROOT/target/smoltcp-myos"
STAMP="$ROOT/target/.smoltcp-myos-version"

sha256() {
  if command -v sha256sum >/dev/null; then sha256sum "$@"; else shasum -a 256 "$@"; fi
}

want="$(cat "$HERE/versions.env" "$HERE"/*.myos.patch | sha256 | cut -d' ' -f1)"
if [[ -f "$STAMP" && "$(cat "$STAMP")" == "$want" && -f "$DEST/Cargo.toml" ]]; then
  exit 0
fi

mkdir -p "$ROOT/target"
tmp="$(mktemp -d "$ROOT/target/smoltcp-myos.XXXXXX")"
trap 'rm -rf "$tmp"' EXIT
crate="$tmp/smoltcp-$SMOLTCP_VERSION.crate"
curl -fsSL --retry 5 --retry-delay 2 -o "$crate" \
  "https://static.crates.io/crates/smoltcp/smoltcp-$SMOLTCP_VERSION.crate"
got="$(sha256 "$crate" | cut -d' ' -f1)"
if [[ "$got" != "$SMOLTCP_SHA256" ]]; then
  echo "error: smoltcp $SMOLTCP_VERSION sha256 $got != pin $SMOLTCP_SHA256" >&2
  exit 1
fi
tar -xzf "$crate" -C "$tmp"
for p in "$HERE"/*.myos.patch; do
  patch -d "$tmp/smoltcp-$SMOLTCP_VERSION" -p1 --batch -s <"$p"
done
rm -rf "$DEST"
mv "$tmp/smoltcp-$SMOLTCP_VERSION" "$DEST"
printf '%s\n' "$want" >"$STAMP"
echo "smoltcp $SMOLTCP_VERSION with myos patches -> $DEST"
