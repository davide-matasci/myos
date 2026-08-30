#!/usr/bin/env bash
set -euo pipefail
REAL="$(rustc +nightly-2026-07-26 --print sysroot)/bin/rustc"

# Cargo invokes this wrapper for host build scripts and proc-macros too.
# Those run without --target and need the normal host sysroot (std for
# x86_64-unknown-linux-gnu). Cross / build-std invocations always pass
# --target and should keep using the myos sysroot overlay.
has_target=0
prev=""
for arg in "$@"; do
  if [[ "$prev" == "--target" ]] || [[ "$arg" == --target=* ]]; then
    has_target=1
    break
  fi
  prev="$arg"
done

if [[ "$has_target" -eq 0 ]]; then
  exec "$REAL" "$@"
fi

SYSROOT="${MYOS_SYSROOT:-$(cd "$(dirname "$0")/.." && pwd)/target/myos-sysroot}"
exec "$REAL" --sysroot="$SYSROOT" "$@"
