#!/usr/bin/env bash
# Thin wrapper; canonical script is ports/lynx/build.sh
exec "$(cd "$(dirname "$0")/.." && pwd)/ports/lynx/build.sh" "$@"
