#!/bin/sh
# The C smokes against mlibc (port.env): scripts/build-c-smokes.sh's second
# set, target/<name>-mlibc-<arch>-unknown-none.
set -euo pipefail
exec "$(cd "$(dirname "$0")/../.." && pwd)/scripts/build-c-smokes.sh" mlibc
