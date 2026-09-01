#!/usr/bin/env bash
# Thin wrapper; canonical script is ports/coreutils/prepare.sh
exec "$(cd "$(dirname "$0")/.." && pwd)/ports/coreutils/prepare.sh" "$@"
