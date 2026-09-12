#!/usr/bin/env bash
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
f="$ROOT/user/uutils-coreutils/src/uu/ln/src/ln.rs"
[[ -f "$f" ]] || exit 0
stamp="$(dirname "$f")/.myos-ln-patch-done"
[[ -f "$stamp" ]] && exit 0
# Portable in-place edit (GNU and BSD sed disagree about -i syntax).
python3 - "$f" <<'PYLN'
from pathlib import Path
import sys

p = Path(sys.argv[1])
text = p.read_text()
text = text.replace(
    '#[cfg(any(unix, target_os = "redox"))]',
    '#[cfg(any(unix, target_os = "redox", target_os = "myos"))]',
)
p.write_text(text)
PYLN
touch "$stamp"
