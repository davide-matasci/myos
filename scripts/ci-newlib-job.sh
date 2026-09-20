#!/usr/bin/env bash
set -euo pipefail
chmod +x scripts/*.sh ports/*/*.sh toolchain/*/*.sh 2>/dev/null || true
./scripts/ci-registry.sh pull newlib || true
./toolchain/newlib/build.sh
./scripts/ci-registry.sh push newlib || true
