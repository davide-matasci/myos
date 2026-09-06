#!/usr/bin/env bash
# Harden uu_ls TextOutput::new against SystemTime underflow on myos.
# myos SystemTime::now() may be near UNIX_EPOCH; `now - 6 months` panicked (exit 101).
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
LS="$ROOT/user/uutils-coreutils/src/uu/ls/src/ls.rs"
[[ -f "$LS" ]] || exit 0
if grep -q 'checked_sub(Duration::new(31_556_952 / 2, 0))' "$LS"; then
  exit 0
fi
python3 - "$LS" <<'PY'
import sys
from pathlib import Path
p = Path(sys.argv[1])
text = p.read_text()
old = """                // Use \"recent\" format for files modified within the last ~0.5 years (31556952s).
                // According to GNU a Gregorian year has 365.2425 * 24 * 60 * 60 == 31556952 seconds on the average.
                recent_time_range: (SystemTime::now() - Duration::new(31_556_952 / 2, 0))
                    ..=SystemTime::now(),"""
new = """                // Use \"recent\" format for files modified within the last ~0.5 years (31556952s).
                // According to GNU a Gregorian year has 365.2425 * 24 * 60 * 60 == 31556952 seconds on the average.
                // checked_sub: myos SystemTime::now can be near UNIX_EPOCH; `now - months` panicked.
                recent_time_range: {
                    let now = SystemTime::now();
                    let start = now
                        .checked_sub(Duration::new(31_556_952 / 2, 0))
                        .unwrap_or(UNIX_EPOCH);
                    start..=now
                },"""
if old not in text:
    raise SystemExit(f"ls recent_time_range block not found in {p}")
p.write_text(text.replace(old, new, 1))
print(f"patched {p}")
PY
