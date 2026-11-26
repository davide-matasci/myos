#!/bin/sh
set -euo pipefail
# The pinned nightly's rustc; on myos (scripts/self-host.sh) the rustc
# app's, which is that one.
if [ -n "${MYOS_SELF_HOST-}" ]; then
  REAL="$(command -v rustc)"
else
  REAL="$(rustc +nightly-2026-07-26 --print sysroot)/bin/rustc"
fi
SYSROOT="${MYOS_SYSROOT:-$(cd "$(dirname "$0")/.." && pwd)/target/myos-sysroot}"
exec "$REAL" --sysroot="$SYSROOT" "$@"
