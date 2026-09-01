#!/usr/bin/env bash
# Thin wrapper; canonical script is ports/ripgrep/build-pcre2.sh
exec "$(cd "$(dirname "$0")/.." && pwd)/ports/ripgrep/build-pcre2.sh" "$@"
