#!/usr/bin/env bash
# Fetch + verify the pinned dropbear source tarball into target/.
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$HERE/../.." && pwd)"
# shellcheck source=versions.env
source "$HERE/versions.env"

TARBALL="$ROOT/target/dropbear-${MYOS_DROPBEAR_VERSION}.tar.bz2"

if [[ -f "$TARBALL" ]] && [[ "$(sha256sum "$TARBALL" | cut -d' ' -f1)" == "$MYOS_DROPBEAR_SHA256" ]]; then
  echo "dropbear tarball already fetched + verified"
else
  echo "fetching dropbear ${MYOS_DROPBEAR_VERSION}"
  curl -fL --retry 3 -o "$TARBALL" "$MYOS_DROPBEAR_URL"
  echo "$MYOS_DROPBEAR_SHA256  $TARBALL" | sha256sum -c -
fi
