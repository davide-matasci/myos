#!/usr/bin/env bash
set -euo pipefail
REAL="$(rustc +nightly-2026-07-26 --print sysroot)/bin/rustc"
SYSROOT="${MYOS_SYSROOT:-$(cd "$(dirname "$0")/.." && pwd)/target/myos-sysroot}"
exec "$REAL" --sysroot="$SYSROOT" "$@"
