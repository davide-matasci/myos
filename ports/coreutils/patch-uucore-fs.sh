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
#
# fsext.rs: every `target_os = "aix",` cfg arm must be accompanied by myos
# (aix is upstream's "no mount listing / unsupported" stand-in). Runs on
# every invocation and self-heals trees stamped by earlier passes:
#   1) drop any standalone `target_os = "myos",` lines left by earlier
#      (line-appending) patch attempts — they can land *outside* a cfg
#      attribute and are syntax errors;
#   2) re-insert myos INLINE, immediately after each `target_os = "aix",`
#      occurrence that is not already followed by a myos arm — matching
#      what the original GNU sed pass produced (the insert stays inside
#      the cfg list whether the attribute is single- or multi-line).
if [[ -f "$fsext" ]]; then
  python3 - "$fsext" <<'PYFSEXT'
from pathlib import Path
import sys

p = Path(sys.argv[1])
lines = p.read_text().splitlines(keepends=True)
kept = "".join(l for l in lines if l.strip() != 'target_os = "myos",')

needle = 'target_os = "aix",'
follow = '\n        target_os = "myos",'
out = []
pos = 0
changed = False
while True:
    idx = kept.find(needle, pos)
    if idx == -1:
        out.append(kept[pos:])
        break
    end = idx + len(needle)
    out.append(kept[pos:end])
    if kept.startswith(follow, end):
        end += len(follow)
    else:
        changed = True
    out.append(follow)
    pos = end

if changed:
    p.write_text("".join(out))
    print("fsext.rs: myos arms (re-)inserted")
PYFSEXT
fi

# fs.rs number_of_links(): the unix arms are `#[cfg(all(\n unix, ...))]`
# (condition split across lines), so the single-line cfg rewrites below
# never reach them. Give myos its own arm instead, right after the windows
# fallback return — exactly what the original GNU sed range-append did.
# Idempotent (two-line guard) and runs before the stamp so trees stamped by
# earlier buggy passes self-heal.
python3 - "$fs" <<'PYFSNLINK'
from pathlib import Path
import sys

p = Path(sys.argv[1])
lines = p.read_text().splitlines(keepends=True)
for i, line in enumerate(lines):
    if (
        line.strip() == '#[cfg(target_os = "myos")]'
        and i + 1 < len(lines)
        and lines[i + 1].strip() == 'return self.0.st_nlink;'
    ):
        raise SystemExit(0)  # already inserted

anchor = None
for i, line in enumerate(lines):
    if '#[cfg(windows)]' in line:
        anchor = i
        break
if anchor is None:
    raise SystemExit("fs.rs: no #[cfg(windows)] line found")
for i in range(anchor + 1, len(lines)):
    if 'return self.0.number_of_links();' in lines[i]:
        lines[i + 1 : i + 1] = [
            '        #[cfg(target_os = "myos")]\n',
            '        return self.0.st_nlink;\n',
        ]
        p.write_text("".join(lines))
        break
else:
    raise SystemExit("fs.rs: number_of_links() return after #[cfg(windows)] not found")
PYFSNLINK

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

touch "$stamp"
