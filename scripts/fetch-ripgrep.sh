#!/usr/bin/env bash
# Thin wrapper; canonical script is ports/ripgrep/fetch.sh
exec "$(cd "$(dirname "$0")/.." && pwd)/ports/ripgrep/fetch.sh" "$@"
