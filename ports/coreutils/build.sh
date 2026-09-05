#!/usr/bin/env bash
# Cross-build uutils coreutils for myos (experimental).
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$HERE/../.." && pwd)"
UUTILS_DIR="$ROOT/user/uutils-coreutils"
UUTILS_TAG="${UUTILS_TAG:-0.10.0}"
TARGET="${MYOS_TARGET:-x86_64-unknown-myos}"
TARGET_JSON="$ROOT/targets/${TARGET}.json"
FEATURES="${COREUTILS_FEATURES:-basename,cat,cp,cut,dirname,du,echo,env,false,head,ln,ls,mkdir,mktemp,mv,printenv,printf,pwd,readlink,realpath,rm,rmdir,seq,sleep,touch,tr,true,uniq,unlink,wc,yes}"
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
  "$ROOT/toolchain/std/build-sysroot.sh"
fi

"$ROOT/ports/coreutils/prepare.sh"

if [[ ! -d "$UUTILS_DIR/.git" ]]; then
  echo "Cloning uutils/coreutils ${UUTILS_TAG}..."
  "$ROOT/scripts/git-retry.sh" clone --depth 1 --branch "$UUTILS_TAG" \
    https://github.com/uutils/coreutils "$UUTILS_DIR"
fi

mkdir -p "$UUTILS_DIR/.cargo"
cp "$ROOT/ports/coreutils/cargo-config.toml" "$UUTILS_DIR/.cargo/config.toml"

if [[ -f "$ROOT/ports/coreutils/uucore-myos.patch" ]] \
  && ! grep -q 'mod myos_argv' "$UUTILS_DIR/src/uucore/src/lib/lib.rs" 2>/dev/null; then
  patch -d "$UUTILS_DIR" -p1 -N --forward <"$ROOT/ports/coreutils/uucore-myos.patch"
fi
"$ROOT/ports/coreutils/patch-uucore-unix.sh"
"$ROOT/ports/coreutils/patch-uucore-fs.sh"
"$ROOT/ports/coreutils/patch-uu-mv.sh"
"$ROOT/ports/coreutils/patch-uu-touch.sh"
"$ROOT/ports/coreutils/patch-uu-ln.sh"
"$ROOT/ports/coreutils/patch-uu-cat.sh"

echo "==> building coreutils for ${TARGET} (${PROFILE}, features=${FEATURES})"
cd "$UUTILS_DIR"
if [[ "$PROFILE" == "release" ]]; then
  export CARGO_PROFILE_RELEASE_LTO=false
fi

# build-std-hello / build-sbase leave RUSTC=myos-rustc.sh in the CI shell; that
# wrapper always uses the myos sysroot and breaks host build scripts. Respect
# .cargo/config.toml myos-rustc-cross.sh instead.
unset RUSTC
cargo +nightly-2026-07-26 build \
  --target "$TARGET_JSON" \
  --no-default-features \
  --features "$FEATURES" \
  --bin coreutils \
  "${PROFILE_ARGS[@]}"

OUT="$UUTILS_DIR/target/${TARGET}/${PROFILE}/coreutils"
echo "Built: $OUT ($(du -h "$OUT" | awk '{print $1}'))"
