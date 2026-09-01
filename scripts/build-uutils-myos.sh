#!/usr/bin/env bash
# Thin wrapper; canonical script is ports/coreutils/build-uutils.sh
exec "$(cd "$(dirname "$0")/.." && pwd)/ports/coreutils/build-uutils.sh" "$@"
