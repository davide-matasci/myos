#!/usr/bin/env bash
# Thin wrapper; canonical script is ports/coreutils/build.sh
exec "$(cd "$(dirname "$0")/.." && pwd)/ports/coreutils/build.sh" "$@"
