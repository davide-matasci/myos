#!/usr/bin/env bash
# Fetch errno + libc from crates.io and apply myos patches into target/patched-crates/.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# shellcheck source=patches/coreutils/versions.env
source "$ROOT/patches/coreutils/versions.env"

PATCHES="$ROOT/patches/coreutils"
DEST="$ROOT/target/patched-crates"
STAMP="$DEST/.coreutils-patches-version"
FETCH_DIR="$ROOT/target/crate-fetch-coreutils"
NIGHTLY="${MYOS_NIGHTLY:-nightly-2026-07-26}"

patch_version_hash() {
  {
    echo "errno=$ERRNO_VERSION libc=$LIBC_VERSION"
    sha256sum "$PATCHES/versions.env"
    sha256sum "$PATCHES/errno/"*
    sha256sum "$PATCHES/libc/"*
  } | sha256sum | awk '{print $1}'
}

if [[ -f "$STAMP" ]] && [[ "$(cat "$STAMP")" == "$(patch_version_hash)" ]] \
  && [[ -f "$DEST/errno-$ERRNO_VERSION/Cargo.toml" ]] \
  && [[ -f "$DEST/libc-$LIBC_VERSION/Cargo.toml" ]]; then
  echo "coreutils patched crates up to date at $DEST"
  exit 0
fi

echo "Fetching errno $ERRNO_VERSION and libc $LIBC_VERSION..."
mkdir -p "$FETCH_DIR"
cat >"$FETCH_DIR/Cargo.toml" <<EOF
[package]
name = "coreutils-crate-fetch"
version = "0.0.0"
edition = "2021"
publish = false

[workspace]

[lib]
path = "lib.rs"

[dependencies]
errno = "= $ERRNO_VERSION"
libc = "= $LIBC_VERSION"
EOF
echo '// fetch-only dummy crate' >"$FETCH_DIR/lib.rs"

( cd "$FETCH_DIR" && cargo "+$NIGHTLY" fetch -q )

find_registry_crate() {
  local name_ver="$1"
  local -a roots=(
    "${CARGO_HOME:-$HOME/.cargo}/registry/src"
    "/usr/local/cargo/registry/src"
  )
  local root dir
  for root in "${roots[@]}"; do
    for dir in "$root"/index.crates.io-*; do
      [[ -d "$dir/$name_ver" ]] || continue
      printf '%s\n' "$dir/$name_ver"
      return 0
    done
  done
  echo "error: could not find $name_ver in cargo registry (run cargo fetch)" >&2
  return 1
}

ERRNO_SRC="$(find_registry_crate "errno-$ERRNO_VERSION")"
LIBC_SRC="$(find_registry_crate "libc-$LIBC_VERSION")"

rm -rf "$DEST"
mkdir -p "$DEST"

apply_patches() {
  local crate_src="$1"
  local crate_name="$2"
  local patch_subdir="$3"
  local out="$DEST/$crate_name"

  echo "==> patching $crate_name"
  cp -a "$crate_src" "$out"
  cp "$PATCHES/$patch_subdir/myos.rs" "$out/src/myos.rs"
  patch -d "$out" -p1 <"$PATCHES/$patch_subdir/lib-rs.patch"
  if [[ -f "$PATCHES/$patch_subdir/sys-rs.patch" ]]; then
    patch -d "$out" -p1 <"$PATCHES/$patch_subdir/sys-rs.patch"
  fi
}

apply_patches "$ERRNO_SRC" "errno-$ERRNO_VERSION" "errno"
apply_patches "$LIBC_SRC" "libc-$LIBC_VERSION" "libc"

patch_version_hash >"$STAMP"
echo "Patched crates ready under $DEST"
