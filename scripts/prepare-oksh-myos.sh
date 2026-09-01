#!/usr/bin/env bash
# Thin wrapper; canonical script is ports/oksh/prepare.sh
exec "$(cd "$(dirname "$0")/.." && pwd)/ports/oksh/prepare.sh" "$@"
