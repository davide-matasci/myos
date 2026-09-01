#!/usr/bin/env bash
# Build uutils coreutils multicall for myos CI and write /c/ manifest.
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# shellcheck source=scripts/myos-c-userspace-lib.sh
source "$ROOT/scripts/myos-c-userspace-lib.sh"

if myos_coreutils_is_current; then
  echo "uutils coreutils up to date"
  exit 0
fi

BINS_FILE="$ROOT/scripts/coreutils-myos/bins.txt"
mapfile -t COREUTILS_BINS <"$BINS_FILE"
FEATURES="${COREUTILS_FEATURES:-echo,true,false,pwd,printf,yes,seq,sleep,wc,head,uniq,cut,tr,env,printenv,basename,dirname}"

build_coreutils() {
  local triple="$1"
  local arch="${triple%%-*}"
  echo "==> uutils coreutils ($triple, features=$FEATURES)"
  COREUTILS_FEATURES="$FEATURES" MYOS_TARGET="$triple" \
    "$ROOT/scripts/build-coreutils-myos.sh" --release
  local bin="$ROOT/user/uutils-coreutils/target/${triple}/release/coreutils"
  cp "$bin" "$ROOT/target/coreutils-${triple}"
  local manifest="$ROOT/target/coreutils-manifest-${arch}.txt"
  : >"$manifest"
  for name in "${COREUTILS_BINS[@]}"; do
    [[ -n "$name" && "$name" != \#* ]] || continue
    echo "$name" >>"$manifest"
  done
  echo "coreutils -> target/coreutils-${triple} ($(du -h "$bin" | awk '{print $1}'), ${#COREUTILS_BINS[@]} names -> ${manifest})"
}

for triple in x86_64-unknown-myos aarch64-unknown-myos riscv64-unknown-myos; do
  build_coreutils "$triple"
done

echo "$(myos_coreutils_version_hash)" >"$MYOS_COREUTILS_VERSION"

# Also build /c/rg (ripgrep+PCRE2) so CI that only invokes uutils still embeds rg.
"$ROOT/scripts/build-ripgrep-myos.sh"
