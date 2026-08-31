#!/usr/bin/env bash
# Minimal uucore myos patches (io only; avoid broad cfg(unix) rewrites).
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
io="$ROOT/user/uutils-coreutils/src/uucore/src/lib/mods/io.rs"

[[ -f "$io" ]] || exit 0

stamp="$(dirname "$io")/.myos-io-patch-done"
[[ -f "$stamp" ]] && exit 0

# Match WASI: convert OwnedFd -> File -> Stdio on myos.
sed -i \
  -e 's/#\[cfg(not(target_os = "wasi"))\]/#[cfg(not(any(target_os = "wasi", target_os = "myos")))]/g' \
  -e 's/#\[cfg(target_os = "wasi")\]/#[cfg(any(target_os = "wasi", target_os = "myos"))]/g' \
  -e 's/#\[cfg(any(unix, target_os = "wasi"))\]/#[cfg(any(unix, target_os = "wasi", target_os = "myos"))]/g' \
  "$io"

touch "$stamp"
