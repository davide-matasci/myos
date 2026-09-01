#!/usr/bin/env bash
# Thin wrapper; canonical script is ports/ubase/build.sh
exec "$(cd "$(dirname "$0")/.." && pwd)/ports/ubase/build.sh" "$@"
