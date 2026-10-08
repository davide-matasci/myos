#!/usr/bin/env bash
# Fetch the pinned Limine binary release (it has limine.c, the tool's
# source, and limine-bios-hdd.h, the BIOS stage it embeds).
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$HERE/../.." && pwd)"
# shellcheck source=ports/limine/versions.env
source "$HERE/versions.env"
SRC="$ROOT/target/limine-src"
STAMP="$SRC/.version"

if [[ -f "$STAMP" && "$(cat "$STAMP")" == "$LIMINE_VERSION" && -f "$SRC/limine.c" ]]; then
  echo "limine $LIMINE_VERSION already present at $SRC"
  exit 0
fi

rm -rf "$SRC"
mkdir -p "$SRC"
tarball="$SRC/limine-binary.tar.gz"
echo "==> fetch limine $LIMINE_VERSION"
curl -fsSL --retry 3 -o "$tarball" \
  "https://github.com/limine-bootloader/limine/releases/download/v${LIMINE_VERSION}/limine-binary.tar.gz"
echo "$LIMINE_SHA256  $tarball" | sha256sum -c - >/dev/null
tar -xzf "$tarball" -C "$SRC" --strip-components=1
rm -f "$tarball"
echo "$LIMINE_VERSION" >"$STAMP"
