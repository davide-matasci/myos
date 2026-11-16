#!/bin/sh
# Thin wrapper; the canonical script is fetch.sh in the lynx port's directory
# (ports/lynx or packages/lynx).
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
# shellcheck source=scripts/myos-c-userspace-lib.sh
. "$ROOT/scripts/myos-c-userspace-lib.sh"
exec "$(myos_port_dir lynx)/fetch.sh" "$@"
