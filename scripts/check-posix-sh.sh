#!/bin/sh
# Every shell script in the repository is POSIX.1-2024 sh, with pipefail
# and without `local`: what OpenBSD ksh (myos's own shell), dash 0.5.13,
# yash and busybox sh all run. This checks it: no bash or ksh shebang,
# nothing shellcheck knows as outside POSIX, no `local`, and a syntax
# check by every POSIX shell found on the host (yash is the strict one).
#
#   scripts/check-posix-sh.sh [FILE...]    (default: every script checked in)
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

if [ $# -eq 0 ]; then
  # The files with a shell's shebang, and the sourced ones (`*.sh`); found
  # without git, which refuses a checkout another user owns (CI's
  # container).
  # shellcheck disable=SC2046
  set -- $({
    grep -rlE --exclude-dir=target --exclude-dir=.git '^#! ?(/usr)?/bin/(env )?[a-z]*sh( |$)' . || true
    find . \( -path ./target -o -path ./.git \) -prune -o -type f -name '*.sh' -print
  } | sed 's|^\./||' | sort -u)
fi
[ $# -gt 0 ] || { echo "error: no script to check" >&2; exit 1; }

command -v shellcheck >/dev/null 2>&1 || { echo "error: shellcheck not found" >&2; exit 1; }

# yash reads a script in the locale's encoding and refuses a byte outside
# it: the comments hold UTF-8.
export LC_ALL=C.UTF-8

status=0
for f in "$@"; do
  case "$(sed -n 1p "$f")" in
    '#!/bin/sh'*) ;;
    '#!'*) echo "$f:1: not a #!/bin/sh script: $(sed -n 1p "$f")"; status=1 ;;
  esac
  if grep -nE '^[[:space:]]*local[[:space:]]' "$f" /dev/null; then
    status=1
  fi
  for sh in yash mksh "busybox sh" "bash --posix"; do
    command -v "${sh%% *}" >/dev/null 2>&1 || continue
    # shellcheck disable=SC2086
    $sh -n "$f" || { echo "$f: syntax error for $sh"; status=1; }
  done
done

# shellcheck's SC3xxx are its "not in POSIX sh" findings. Two are in
# POSIX.1-2024, which it does not know yet: SC3040 (set -o pipefail) and
# SC3013 (test's -nt and -ot).
# (shellcheck exits 1 for its other findings; one grep or the other finds
# nothing when all is well.)
if { shellcheck -s sh -S warning -f gcc "$@" || true; } | { grep -E '\[SC3[0-9]+\]' || true; } | grep -vE '\[SC30(40|13)\]'; then
  status=1
fi
[ $status -eq 0 ] && echo "check-posix-sh: $# scripts, POSIX sh"
exit $status
