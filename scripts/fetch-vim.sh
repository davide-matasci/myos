#!/usr/bin/env bash
# Thin wrapper; the canonical script is fetch.sh in the vim port's directory
# (ports/vim or packages/vim).
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
# shellcheck source=scripts/myos-c-userspace-lib.sh
source "$ROOT/scripts/myos-c-userspace-lib.sh"
exec "$(myos_port_dir vim)/fetch.sh" "$@"
