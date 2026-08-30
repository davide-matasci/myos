#!/usr/bin/env bash
# Cross-build uutils coreutils for myos (experimental).
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
UUTILS_DIR="$ROOT/user/uutils-coreutils"
UUTILS_TAG="${UUTILS_TAG:-0.10.0}"
TARGET="${MYOS_TARGET:-x86_64-unknown-myos}"
TARGET_JSON="$ROOT/targets/${TARGET}.json"
FEATURES="${COREUTILS_FEATURES:-echo,true,false}"
PROFILE="${1:-dev}"

if [[ "$PROFILE" == "--release" ]]; then
  PROFILE=release
  PROFILE_ARGS=(--release)
else
  PROFILE=debug
  PROFILE_ARGS=()
fi

if [[ ! -d "$ROOT/target/myos-sysroot/lib/rustlib/${TARGET}/lib" ]]; then
  echo "Building myos sysroot first..."
  "$ROOT/scripts/build-sysroot.sh"
fi

"$ROOT/scripts/prepare-coreutils-patches.sh"

if [[ ! -d "$UUTILS_DIR/.git" ]]; then
  echo "Cloning uutils/coreutils ${UUTILS_TAG}..."
  git clone --depth 1 --branch "$UUTILS_TAG" \
    https://github.com/uutils/coreutils "$UUTILS_DIR"
fi

mkdir -p "$UUTILS_DIR/.cargo"
cp "$ROOT/vendor/coreutils-port/cargo-config.toml" "$UUTILS_DIR/.cargo/config.toml"

if [[ -f "$ROOT/patches/coreutils/uucore-myos.patch" ]]; then
  patch -d "$UUTILS_DIR" -p1 -N <"$ROOT/patches/coreutils/uucore-myos.patch" || true
fi

echo "==> building coreutils for ${TARGET} (${PROFILE}, features=${FEATURES})"
cd "$UUTILS_DIR"
if [[ "$PROFILE" == "release" ]]; then
  export CARGO_PROFILE_RELEASE_LTO=false
fi
cargo +nightly-2026-07-26 build \
  --target "$TARGET_JSON" \
  --no-default-features \
  --features "$FEATURES" \
  --bin coreutils \
  "${PROFILE_ARGS[@]}"

OUT="$UUTILS_DIR/target/${TARGET}/${PROFILE}/coreutils"
echo "Built: $OUT ($(du -h "$OUT" | awk '{print $1}'))"
