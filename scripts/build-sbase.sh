#!/bin/sh
# Thin wrapper; canonical script is ports/sbase/build.sh
exec "$(cd "$(dirname "$0")/.." && pwd)/ports/sbase/build.sh" "$@"
