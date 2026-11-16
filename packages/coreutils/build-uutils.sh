#!/bin/sh
# Build uutils coreutils multicall for myos CI and write /c/ manifest.
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
ROOT="$(cd "$HERE/../.." && pwd)"
# shellcheck source=scripts/myos-c-userspace-lib.sh
. "$ROOT/scripts/myos-c-userspace-lib.sh"

if myos_coreutils_is_current; then
  echo "uutils coreutils up to date"
  exit 0
fi

BINS_FILE="$ROOT/packages/coreutils/bins.txt"
# The file's lines, one per line, and how many they are.
COREUTILS_BINS=""
COREUTILS_BINS_COUNT=0
while IFS= read -r line; do
  COREUTILS_BINS="$COREUTILS_BINS$line
"
  COREUTILS_BINS_COUNT=$((COREUTILS_BINS_COUNT + 1))
done <"$BINS_FILE"
FEATURES="${COREUTILS_FEATURES:-basename,cat,cp,cut,dirname,du,echo,env,false,head,ln,ls,mkdir,mktemp,mv,printenv,printf,pwd,readlink,realpath,rm,rmdir,seq,sleep,touch,tr,true,uniq,unlink,wc,yes}"

build_coreutils() (
  triple="$1"
  arch="${triple%%-*}"
  echo "==> uutils coreutils ($triple, features=$FEATURES)"
  COREUTILS_FEATURES="$FEATURES" MYOS_TARGET="$triple" \
    "$ROOT/packages/coreutils/build.sh" --release
  bin="$ROOT/user/uutils-coreutils/target/${triple}/release/coreutils"
  cp "$bin" "$ROOT/target/coreutils-${triple}"
  manifest="$ROOT/target/coreutils-manifest-${arch}.txt"
  : >"$manifest"
  while IFS= read -r name; do
    case "$name" in '' | \#*) continue ;; esac
    printf '%s\n' "$name" >>"$manifest"
  done <<EOF
$COREUTILS_BINS
EOF
  echo "coreutils -> target/coreutils-${triple} ($(du -h "$bin" | awk '{print $1}'), $COREUTILS_BINS_COUNT names -> ${manifest})"
)

for triple in x86_64-unknown-myos aarch64-unknown-myos riscv64-unknown-myos; do
  build_coreutils "$triple"
done

echo "$(myos_coreutils_version_hash)" >"$MYOS_COREUTILS_VERSION"
