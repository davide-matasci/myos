#!/usr/bin/env bash
# Thin wrapper; canonical script is ports/ripgrep/prepare.sh
exec "$(cd "$(dirname "$0")/.." && pwd)/ports/ripgrep/prepare.sh" "$@"
