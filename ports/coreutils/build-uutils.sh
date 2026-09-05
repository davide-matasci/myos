#!/usr/bin/env bash
# Build uutils coreutils multicall for myos CI and write /c/ manifest.
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$HERE/../.." && pwd)"
# shellcheck source=scripts/myos-c-userspace-lib.sh
source "$ROOT/scripts/myos-c-userspace-lib.sh"

build_follow_on_ports() {
  # rg + tcc have their own stamps. The CI build job already runs this script
  # before GHCR push; chaining tcc here means a successful build can
  # `registry push tcc` without editing .github/workflows (needs `workflow` scope).
  # tcc also writes target/coreutils-tcc-* so ci-build.tar's coreutils-* glob packs it.
  "$ROOT/ports/ripgrep/build.sh"
  "$ROOT/ports/tcc/build.sh"
}

if myos_coreutils_is_current; then
  echo "uutils coreutils up to date"
  build_follow_on_ports
  exit 0
fi

BINS_FILE="$ROOT/ports/coreutils/bins.txt"
mapfile -t COREUTILS_BINS <"$BINS_FILE"
FEATURES="${COREUTILS_FEATURES:-base32,base64,basename,basenc,cat,cksum,b2sum,md5sum,sha1sum,sha224sum,sha256sum,sha384sum,sha512sum,comm,cp,csplit,cut,date,dd,dir,dircolors,dirname,du,echo,env,expand,factor,false,fmt,fold,head,join,link,ln,ls,mkdir,mktemp,mv,nl,numfmt,od,paste,pathchk,pr,printenv,printf,ptx,pwd,readlink,realpath,rm,rmdir,seq,shred,shuf,sleep,sort,sum,tee,touch,tr,true,truncate,tsort,unexpand,uniq,unlink,vdir,wc,yes,arch,hostname,nproc,uname}"

build_coreutils() {
  local triple="$1"
  local arch="${triple%%-*}"
  echo "==> uutils coreutils ($triple, features=$FEATURES)"
  COREUTILS_FEATURES="$FEATURES" MYOS_TARGET="$triple" \
    "$ROOT/ports/coreutils/build.sh" --release
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

# Also build /c/rg and tcc so CI that only invokes uutils still embeds them.
build_follow_on_ports
