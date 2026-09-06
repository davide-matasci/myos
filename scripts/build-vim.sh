#!/usr/bin/env bash
# Thin wrapper; canonical script is ports/vim/build.sh
exec "$(cd "$(dirname "$0")/.." && pwd)/ports/vim/build.sh" "$@"
