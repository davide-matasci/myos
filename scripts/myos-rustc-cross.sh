#!/usr/bin/env bash
# rustc wrapper for cross-compiling userland (e.g. uutils) to *-unknown-myos.
# Host build scripts, proc-macros, and host-side helper crates must keep the
# normal linux-gnu sysroot; only explicit cross compiles use the myos overlay.
set -euo pipefail
REAL="$(rustc +nightly-2026-07-26 --print sysroot)/bin/rustc"

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
prev=""
for arg in "$@"; do
  if [[ "$prev" == "--target" ]] || [[ "$arg" == --target=* ]]; then
    use_myos_sysroot=1
    break
  fi
  prev="$arg"
done

if [[ "$use_myos_sysroot" -eq 0 ]]; then
  exec "$REAL" "$@"
fi

filtered=()
skip_next=0
for arg in "$@"; do
  if [[ "$skip_next" -eq 1 ]]; then
    skip_next=0
    continue
  fi
  case "$arg" in
    --sysroot) skip_next=1 ;;
    --sysroot=*) ;;
    *) filtered+=("$arg") ;;
  esac
done

SYSROOT="${MYOS_SYSROOT:-$(cd "$(dirname "$0")/.." && pwd)/target/myos-sysroot}"
exec "$REAL" --sysroot="$SYSROOT" "${filtered[@]}"
