#!/usr/bin/env bash
# Quick, reliable local boot-CI run. Fixes the two chronic failure modes:
# 1. OOM: OpenClaw's exec wrapper sets oom_score_adj=1000 on children, so a
#    2 GiB TCG QEMU gets SIGKILLed under memory pressure (swap-less host).
#    This wrapper lowers its own oom_score_adj (inherited by QEMU) and the
#    host now has 4G swap.
# 2. Stale state: leftover qemu holds target/bios.img write lock or host
#    :2323/:8765, silently wedging the run. Preflight cleans + checks.
#
# Usage: scripts/local-ci.sh [bios|uefi|aarch64|riscv64]   (default bios)
set -euo pipefail
MODE="${1:-bios}"

# OOM protection for the whole tree we spawn (needs root; ignore if not).
if [ -w /proc/self/oom_score_adj ]; then
    echo -500 > /proc/self/oom_score_adj 2>/dev/null || true
fi

# Preflight: kill stale QEMU from aborted runs.
pkill -9 -f qemu-system 2>/dev/null || true
sleep 1

# Port guards mirror src/main.rs guestfwd/hostfwd conditions.
if ss -tln | grep -q ':2323 '; then
    echo "WARNING: host :2323 busy -> listen smoke hostfwd will be skipped" >&2
fi
if ss -tln | grep -q ':8765 '; then
    echo "NOTE: host :8765 busy -> guestfwd will be skipped" >&2
fi

cd "$(dirname "$0")/.."
exec nice -n 5 cargo +nightly-2026-07-26 run --quiet -- "$MODE" --ci
