#!/usr/bin/env bash
# Thin wrapper; canonical script is ports/git/build.sh
exec "$(cd "$(dirname "$0")/.." && pwd)/ports/git/build.sh" "$@"
