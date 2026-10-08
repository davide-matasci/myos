#!/usr/bin/env bash
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
f="$ROOT/user/uutils-coreutils/src/uu/cat/src/platform/mod.rs"
[[ -f "$f" ]] || exit 0
stamp="$(dirname "$f")/.myos-cat-patch-done"
[[ -f "$stamp" ]] && exit 0
# Portable in-place edit (GNU and BSD sed disagree about -i syntax).
python3 - "$f" <<'PYCAT'
from pathlib import Path
import sys

p = Path(sys.argv[1])
text = p.read_text()
text = text.replace(
    '#[cfg(target_os = "wasi")]',
    '#[cfg(any(target_os = "wasi", target_os = "myos"))]',
)
p.write_text(text)
PYCAT
touch "$stamp"
