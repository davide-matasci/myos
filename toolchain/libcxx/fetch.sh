#!/usr/bin/env bash
# Fetch llvm-project at its pinned release into target/libcxx-src, only
# what the runtimes build reads (a sparse checkout of a shallow clone):
# runtimes/ (the CMake entry), libcxx, libcxxabi, libunwind, cmake/ and
# llvm/cmake (the modules the runtimes include).
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$HERE/../.." && pwd)"
# shellcheck source=toolchain/libcxx/versions.env
source "$HERE/versions.env"
SRC="$ROOT/target/libcxx-src"

if [[ -d "$SRC/.git" ]] && [[ "$(git -C "$SRC" rev-parse HEAD)" == "$LLVM_REV" ]]; then
  exit 0
fi
echo "==> fetch llvm-project runtimes ($LLVM_TAG)"
rm -rf "$SRC"
"$ROOT/scripts/git-retry.sh" clone --depth 1 --branch "$LLVM_TAG" --filter=blob:none --sparse \
  https://github.com/llvm/llvm-project.git "$SRC"
git -C "$SRC" sparse-checkout set runtimes libcxx libcxxabi libunwind cmake llvm/cmake llvm/utils/llvm-lit
got="$(git -C "$SRC" rev-parse HEAD)"
if [[ "$got" != "$LLVM_REV" ]]; then
  echo "error: llvm-project $LLVM_TAG is $got, want $LLVM_REV" >&2
  exit 1
fi
