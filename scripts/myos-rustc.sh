#!/usr/bin/env bash
set -euo pipefail
REAL="$(rustc +nightly-2026-07-26 --print sysroot)/bin/rustc"

# Host-side build scripts and proc-macros always need the normal toolchain sysroot.
host_build=0
prev=""
for arg in "$@"; do
  if [[ "$prev" == "--crate-name" && "$arg" == build_script_build ]]; then
    host_build=1
    break
  fi
  if [[ "$prev" == "--crate-type" && "$arg" == proc-macro ]]; then
    host_build=1
    break
  fi
  case "$arg" in
    --crate-name=build_script_build) host_build=1 ;;
    --crate-type=proc-macro) host_build=1 ;;
  esac
  prev="$arg"
done

if [[ "$host_build" -eq 1 ]]; then
  exec "$REAL" "$@"
fi

use_myos_sysroot=0
if [[ -n "${MYOS_RUSTC_FORCE_SYSROOT:-}" ]]; then
  use_myos_sysroot=1
else
  prev=""
  for arg in "$@"; do
    if [[ "$prev" == "--target" ]] || [[ "$arg" == --target=* ]]; then
      use_myos_sysroot=1
      break
    fi
    prev="$arg"
  done
fi

if [[ "$use_myos_sysroot" -eq 1 ]]; then
  SYSROOT="${MYOS_SYSROOT:-$(cd "$(dirname "$0")/.." && pwd)/target/myos-sysroot}"
  exec "$REAL" --sysroot="$SYSROOT" "$@"
fi

exec "$REAL" "$@"
