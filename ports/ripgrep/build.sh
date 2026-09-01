#!/usr/bin/env bash
# Cross-build full ripgrep (with PCRE2) for myos userspace → target/rg-*-unknown-myos.
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$HERE/../.." && pwd)"
# shellcheck source=scripts/myos-c-userspace-lib.sh
source "$ROOT/scripts/myos-c-userspace-lib.sh"
# shellcheck source=ports/ripgrep/versions.env
source "$ROOT/ports/ripgrep/versions.env"

NIGHTLY="${MYOS_NIGHTLY:-nightly-2026-07-26}"
# Ensure rust-lld is on PATH (dtolnay toolchain layout).
RUST_LLD_BIN="$(rustc "+$NIGHTLY" --print sysroot)/lib/rustlib/$(rustc "+$NIGHTLY" -vV | awk '/host:/{print $2}')/bin"
export PATH="$ROOT/target/newlib-bin:$RUST_LLD_BIN:$PATH"

if myos_ripgrep_is_current; then
  echo "ripgrep ELFs up to date"
  for triple in x86_64-unknown-myos aarch64-unknown-myos riscv64-unknown-myos; do
    src="$ROOT/target/rg-${triple}"
    alias="$ROOT/target/coreutils-rg-${triple}"
    if [[ -f "$src" && ! -f "$alias" ]]; then
      cp "$src" "$alias"
    fi
  done
  exit 0
fi

if [[ ! -d "$ROOT/target/myos-sysroot/lib/rustlib/x86_64-unknown-myos/lib" ]]; then
  echo "Building myos sysroot first..."
  "$ROOT/toolchain/std/build-sysroot.sh"
fi

"$ROOT/ports/ripgrep/build-pcre2.sh"
"$ROOT/ports/ripgrep/prepare.sh"

RG="$ROOT/target/ripgrep-src"
TARGET_JSON_DIR="$ROOT/targets"

build_one() {
  local triple="$1"
  local arch="${triple%%-*}"
  local target_json="$TARGET_JSON_DIR/${triple}.json"
  local target_dir="$ROOT/target/ripgrep-build-${triple}"
  local out="$ROOT/target/rg-${triple}"
  local pcre2="$ROOT/target/pcre2-${arch}"

  # Scrub stale pcre2-sys build-script outputs so PCRE2_LIB_DIR link lines apply.
  rm -rf "$target_dir"/*/release-myos/build/pcre2-sys-*          "$target_dir"/release-myos/build/pcre2-sys-* 2>/dev/null || true
  echo "==> ripgrep ($triple, features=pcre2)"
  (
    cd "$RG"
    unset RUSTC
    export PCRE2_LIB_DIR="$pcre2/lib"
    export PCRE2_INCLUDE_DIR="$pcre2/include"
    export PCRE2_SYS_STATIC=1
    # Host build scripts still need a working CC; pcre2-sys short-circuits via PCRE2_LIB_DIR.
    cargo "+$NIGHTLY" build \
      --profile release-myos \
      --features pcre2 \
      --target "$target_json" \
      --bin rg \
      --target-dir "$target_dir"
  )
  local bin="$target_dir/${triple}/release-myos/rg"
  if [[ ! -f "$bin" ]]; then
    # cargo may place profile dir as release-myos or under release with rename
    bin="$target_dir/${triple}/release/rg"
  fi
  if [[ ! -f "$bin" ]]; then
    echo "error: rg binary missing under $target_dir/${triple}" >&2
    find "$target_dir/${triple}" -name rg -type f 2>/dev/null | head
    exit 1
  fi
  cp "$bin" "$out"
  # Packed by CI via the existing `target/coreutils-*` glob (workflow edits
  # need `workflow` OAuth scope; boot matrix rebuilds aarch64/riscv kernels).
  cp "$bin" "$ROOT/target/coreutils-rg-${triple}"
  echo "rg -> $out ($(du -h "$out" | awk '{print $1}'))"
}

for triple in x86_64-unknown-myos aarch64-unknown-myos riscv64-unknown-myos; do
  build_one "$triple"
done

echo "$(myos_ripgrep_version_hash)" >"$MYOS_RIPGREP_VERSION"
