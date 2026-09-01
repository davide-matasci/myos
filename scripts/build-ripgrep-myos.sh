#!/usr/bin/env bash
# Thin wrapper; canonical script is ports/ripgrep/build.sh
exec "$(cd "$(dirname "$0")/.." && pwd)/ports/ripgrep/build.sh" "$@"
