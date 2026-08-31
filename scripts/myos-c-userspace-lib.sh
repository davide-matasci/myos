#!/usr/bin/env bash
# Shared version stamps for newlib + C userspace smoke ELFs (CI cache invalidation).
set -euo pipefail

MYOS_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
export MYOS_ROOT

MYOS_NEWLIB_TAG="${NEWLIB_TAG:-newlib-4.4.0}"
MYOS_NEWLIB_VERSION="$MYOS_ROOT/target/.myos-newlib-version"
MYOS_C_HELLO_VERSION="$MYOS_ROOT/target/.myos-c-hello-version"
MYOS_SBASE_VERSION="$MYOS_ROOT/target/.myos-sbase-version"

MYOS_SBASE_MANIFEST="$MYOS_ROOT/target/sbase-manifest-x86_64.txt"
MYOS_SBASE_MIN_BUILT=90

myos_newlib_version_hash() {
  local h
  h="$(
    {
      echo "newlib_tag=$MYOS_NEWLIB_TAG"
      find "$MYOS_ROOT/newlib/libgloss/myos" -type f -print0 2>/dev/null \
        | sort -z | xargs -0 sha256sum
      sha256sum "$MYOS_ROOT/scripts/patch-newlib-myos.sh"
      sha256sum "$MYOS_ROOT/scripts/build-libgloss-myos.sh"
      sha256sum "$MYOS_ROOT/scripts/newlib-tool-wrappers.sh"
    } | sha256sum | awk '{print $1}'
  )"
  printf '%s' "$h"
}

myos_newlib_prefix_ok() {
  local arch="$1"
  local triple="${arch}-unknown-myos"
  local prefix="$MYOS_ROOT/target/newlib-${arch}/${triple}/lib"
  [[ -f "$prefix/libc.a" && -f "$prefix/libgloss.a" && -f "$prefix/crt0.o" ]]
}

myos_newlib_is_current() {
  [[ -f "$MYOS_NEWLIB_VERSION" ]] \
    && [[ "$(cat "$MYOS_NEWLIB_VERSION")" == "$(myos_newlib_version_hash)" ]] \
    && myos_newlib_prefix_ok x86_64 \
    && myos_newlib_prefix_ok aarch64 \
    && myos_newlib_prefix_ok riscv64
}

myos_c_hello_version_hash() {
  local h
  h="$(
    {
      myos_newlib_version_hash
      sha256sum "$MYOS_ROOT/c/hello.c"
      sha256sum "$MYOS_ROOT/scripts/build-c-hello.sh"
    } | sha256sum | awk '{print $1}'
  )"
  printf '%s' "$h"
}

myos_c_hello_is_current() {
  [[ -f "$MYOS_C_HELLO_VERSION" ]] \
    && [[ "$(cat "$MYOS_C_HELLO_VERSION")" == "$(myos_c_hello_version_hash)" ]] \
    && [[ -f "$MYOS_ROOT/target/c-hello-x86_64-unknown-none" ]] \
    && [[ -f "$MYOS_ROOT/target/c-hello-aarch64-unknown-none" ]] \
    && [[ -f "$MYOS_ROOT/target/c-hello-riscv64-unknown-none" ]]
}

myos_sbase_version_hash() {
  local h
  h="$(
    {
      myos_newlib_version_hash
      sha256sum "$MYOS_ROOT/scripts/build-sbase.sh"
      sha256sum "$MYOS_ROOT/scripts/prepare-sbase-myos.sh"
      sha256sum "$MYOS_ROOT/scripts/fetch-sbase.sh"
      sha256sum "$MYOS_ROOT/scripts/sbase-myos/bins.txt"
      find "$MYOS_ROOT/scripts/sbase-myos" -type f -print0 2>/dev/null \
        | sort -z | xargs -0 sha256sum
    } | sha256sum | awk '{print $1}'
  )"
  printf '%s' "$h"
}

myos_sbase_manifest_count() {
  local manifest="$1"
  [[ -f "$manifest" ]] || return 1
  wc -l <"$manifest" | tr -d ' '
}

myos_sbase_is_current() {
  local arch manifest count
  [[ -f "$MYOS_SBASE_VERSION" ]] \
    && [[ "$(cat "$MYOS_SBASE_VERSION")" == "$(myos_sbase_version_hash)" ]] \
    || return 1
  for arch in x86_64 aarch64 riscv64; do
    manifest="$MYOS_ROOT/target/sbase-manifest-${arch}.txt"
    count="$(myos_sbase_manifest_count "$manifest")" || return 1
    if ((count < MYOS_SBASE_MIN_BUILT)); then
      return 1
    fi
    while IFS= read -r line; do
      [[ -n "$line" ]] || continue
      local path="${line#*:}"
      [[ -f "$path" ]] || return 1
    done <"$manifest"
  done
}
