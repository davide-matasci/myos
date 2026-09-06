#!/usr/bin/env bash
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
f="$ROOT/user/uutils-coreutils/src/uu/touch/src/touch.rs"
[[ -f "$f" ]] || exit 0
stamp="$(dirname "$f")/.myos-touch-patch-done"
[[ -f "$stamp" ]] && exit 0
if grep -q 'target_os = "myos"' "$f"; then
  touch "$stamp"
  exit 0
fi
python3 - "$f" <<'PY'
from pathlib import Path
import sys
p = Path(sys.argv[1])
text = p.read_text()
old = """    #[cfg(target_os = \"android\")]
    {
        Ok(PathBuf::from(\"/proc/self/fd/1\"))
    }
    #[cfg(windows)]
"""
new = """    #[cfg(target_os = \"android\")]
    {
        Ok(PathBuf::from(\"/proc/self/fd/1\"))
    }
    #[cfg(target_os = \"myos\")]
    {
        Ok(PathBuf::from(\"/dev/stdout\"))
    }
    #[cfg(windows)]
"""
# fix escaping for the actual file content
old = old.replace('\\"', '"')
new = new.replace('\\"', '"')
if old not in text:
    raise SystemExit('touch pathbuf_from_stdout pattern not found')
p.write_text(text.replace(old, new, 1))
print('touch myos pathbuf_from_stdout applied')
PY
touch "$stamp"
