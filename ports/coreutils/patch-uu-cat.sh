#!/usr/bin/env bash
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
f="$ROOT/user/uutils-coreutils/src/uu/cat/src/platform/mod.rs"
[[ -f "$f" ]] || exit 0
stamp="$(dirname "$f")/.myos-cat-patch-done"
[[ -f "$stamp" ]] && exit 0
sed -i 's/#\[cfg(target_os = "wasi")\]/#[cfg(any(target_os = "wasi", target_os = "myos"))]/' "$f"
touch "$stamp"
