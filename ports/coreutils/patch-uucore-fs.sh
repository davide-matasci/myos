#!/usr/bin/env bash
# uucore fs.rs: treat myos like unix for FileInformation (rustix stat backend).
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$HERE/../.." && pwd)"
fs="$ROOT/user/uutils-coreutils/src/uucore/src/lib/features/fs.rs"
fsext="$ROOT/user/uutils-coreutils/src/uucore/src/lib/features/fsext.rs"

[[ -f "$fs" ]] || exit 0

# Portable in-place edits: GNU and BSD sed disagree about -i syntax, so use
# python3 (same convention as prepare.sh and toolchain/newlib/patch.sh).

# fsext.rs: every cfg list naming aix must also name myos (aix is upstream's
# "no mount listing" stand-in). Runs on every invocation and is idempotent —
# it must not be gated on the stamp below, or a tree stamped by an older
# buggy patch pass never gets healed.
if [[ -f "$fsext" ]]; then
  python3 - "$fsext" <<'PYFSEXT'
from pathlib import Path
import sys

p = Path(sys.argv[1])
lines = p.read_text().splitlines(keepends=True)
out = []
changed = False
for i, line in enumerate(lines):
    out.append(line)
    if 'target_os = "aix",' in line and 'target_os = "myos"' not in line:
        nxt = lines[i + 1] if i + 1 < len(lines) else ''
        if 'target_os = "myos"' not in nxt:
            out.append('        target_os = "myos",\n')
            changed = True
if changed:
    p.write_text("".join(out))
    print("fsext.rs: myos arms added")
PYFSEXT
fi

stamp="$(dirname "$fs")/.myos-fs-patch-done"
[[ -f "$stamp" ]] && exit 0

python3 - "$fs" <<'PYFS'
from pathlib import Path
import sys

p = Path(sys.argv[1])
text = p.read_text()
repls = [
    ('#[cfg(unix)]', '#[cfg(any(unix, target_os = "myos"))]'),
    ('#[cfg(not(unix))]', '#[cfg(not(any(unix, target_os = "myos")))]'),
    ('#[cfg(all(unix,', '#[cfg(all(any(unix, target_os = "myos"),'),
    ('#[cfg(any(unix,', '#[cfg(any(unix, target_os = "myos",'),
]
for old, new in repls:
    text = text.replace(old, new)
p.write_text(text)
PYFS

python3 - "$fs" <<'PYFSNLINK'
from pathlib import Path
import sys

p = Path(sys.argv[1])
text = p.read_text()
if 'return self.0.st_nlink;' in text:
    raise SystemExit(0)
lines = text.splitlines(keepends=True)
insert = [
    '        #[cfg(target_os = "myos")]\n',
    '        return self.0.st_nlink;\n',
]
in_windows = False
for i, line in enumerate(lines):
    if '#[cfg(windows)]' in line:
        in_windows = True
    if in_windows and 'return self.0.number_of_links();' in line:
        lines[i + 1 : i + 1] = insert
        break
else:
    raise SystemExit("fs.rs: number_of_links() return after #[cfg(windows)] not found")
p.write_text("".join(lines))
PYFSNLINK

touch "$stamp"
