#!/usr/bin/env bash
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
f="$ROOT/user/uutils-coreutils/src/uu/ln/src/ln.rs"
[[ -f "$f" ]] || exit 0
stamp="$(dirname "$f")/.myos-ln-patch-done"
[[ -f "$stamp" ]] && exit 0
sed -i 's/#\[cfg(any(unix, target_os = "redox"))\]/#[cfg(any(unix, target_os = "redox", target_os = "myos"))]/' "$f"
touch "$stamp"
