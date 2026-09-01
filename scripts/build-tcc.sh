#!/usr/bin/env bash
# Thin wrapper; canonical script is ports/tcc/build.sh
exec "$(cd "$(dirname "$0")/.." && pwd)/ports/tcc/build.sh" "$@"
