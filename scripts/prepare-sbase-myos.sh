#!/bin/sh
# Thin wrapper; canonical script is ports/sbase/prepare.sh
exec "$(cd "$(dirname "$0")/.." && pwd)/ports/sbase/prepare.sh" "$@"
