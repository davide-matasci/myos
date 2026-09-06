#!/usr/bin/env bash
# Treat myos like wasi for rename_symlink_fallback (no unix symlink rename helpers).
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
mv="$ROOT/user/uutils-coreutils/src/uu/mv/src/mv.rs"
[[ -f "$mv" ]] || exit 0
stamp="$(dirname "$mv")/.myos-mv-patch-done"
[[ -f "$stamp" ]] && exit 0
if grep -q 'target_os = "myos"' "$mv" && grep -q 'rename_symlink_fallback' "$mv"; then
  touch "$stamp"
  exit 0
fi
sed -i 's/#\[cfg(target_os = "wasi")\]/#[cfg(any(target_os = "wasi", target_os = "myos"))]/' "$mv"
touch "$stamp"
