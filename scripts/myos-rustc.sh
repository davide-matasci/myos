#!/usr/bin/env bash
set -euo pipefail
REAL="$(rustc +nightly-2026-07-26 --print sysroot)/bin/rustc"

# Cargo invokes this wrapper for host build scripts and proc-macros too.
# Only swap in the myos sysroot when actually cross-compiling to *-unknown-myos.
myos_target=0
prev=""
for arg in "$@"; do
  if [[ "$prev" == "--target" ]] && [[ "$arg" == *unknown-myos* ]]; then
    myos_target=1
  fi
  case "$arg" in
    --target=*unknown-myos*) myos_target=1 ;;
  esac
  prev="$arg"
done

if [[ "$myos_target" -eq 1 ]]; then
  SYSROOT="${MYOS_SYSROOT:-$(cd "$(dirname "$0")/.." && pwd)/target/myos-sysroot}"
  exec "$REAL" --sysroot="$SYSROOT" "$@"
fi

exec "$REAL" "$@"
