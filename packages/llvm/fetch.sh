#!/usr/bin/env bash
# Fetch llvm-project's compiler half (llvm, clang, lld) into
# target/llvm-src, at toolchain/libcxx's pin: one LLVM version in the
# tree, the runtimes' and the compiler's.
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$HERE/../.." && pwd)"
# shellcheck source=toolchain/libcxx/versions.env
source "$ROOT/toolchain/libcxx/versions.env"
SRC="$ROOT/target/llvm-src"

if [[ -d "$SRC/.git" ]] && [[ "$(git -C "$SRC" rev-parse HEAD)" == "$LLVM_REV" ]] && [[ -d "$SRC/lld" ]]; then
  exit 0
fi
echo "==> fetch llvm-project: llvm, clang, lld ($LLVM_TAG)"
rm -rf "$SRC"
"$ROOT/scripts/git-retry.sh" clone --depth 1 --branch "$LLVM_TAG" --filter=blob:none --sparse \
  https://github.com/llvm/llvm-project.git "$SRC"
# libunwind/include: lld's Mach-O port reads its compact unwind encoding
# (the port is compiled with the others). Not the three test suites, 1.2 GB
# of the 1.6 the rest would be, which a build without tests never reads.
git -C "$SRC" sparse-checkout set --no-cone /llvm/ /clang/ /lld/ /cmake/ /third-party/ /libunwind/include/ \
  '!/llvm/test/' '!/clang/test/' '!/lld/test/'
got="$(git -C "$SRC" rev-parse HEAD)"
if [[ "$got" != "$LLVM_REV" ]]; then
  echo "error: llvm-project $LLVM_TAG is $got, want $LLVM_REV" >&2
  exit 1
fi
