#!/usr/bin/env bash
# Thin wrapper; the canonical script is build.sh in the git port's directory
# (ports/git or packages/git).
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
# shellcheck source=scripts/myos-c-userspace-lib.sh
source "$ROOT/scripts/myos-c-userspace-lib.sh"
exec "$(myos_port_dir git)/build.sh" "$@"
