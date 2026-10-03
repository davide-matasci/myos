#!/usr/bin/env bash
# The C smoke programs (user/c): c-hello + socket_smoke, the TCP listen,
# pty and urandom boot-CI smokes, for the three arches. One stamp for all
# of them (the `c-smokes` port of scripts/ports.sh); each script skips
# itself or is cheap when current.
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# shellcheck source=scripts/myos-c-userspace-lib.sh
source "$ROOT/scripts/myos-c-userspace-lib.sh"
if myos_c_smokes_is_current; then
  echo "c smokes up to date"
  exit 0
fi
"$ROOT/scripts/build-c-hello.sh"
"$ROOT/scripts/build-tcp-listen-smoke.sh"
"$ROOT/scripts/build-pty-smoke.sh"
"$ROOT/scripts/build-urandom-smoke.sh"
myos_c_smokes_version_hash > "$MYOS_C_SMOKES_VERSION"
echo "c smokes -> target/{c-hello,c-socket_smoke,tcp-listen-smoke,pty-smoke,urandom-smoke}-<arch>-unknown-none"
