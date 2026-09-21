#!/usr/bin/env bash
# Quick, reliable local boot-CI run. Fixes the chronic failure modes:
# 1. OOM: OpenClaw's exec wrapper sets oom_score_adj=1000 on children, so a
#    2 GiB TCG QEMU gets SIGKILLed under memory pressure. This wrapper
#    lowers its own oom_score_adj (inherited by QEMU); the host has 4G swap.
# 2. Stale state: leftover QEMU holds target/bios.img write lock or host
#    :2323/:8765. Preflight cleans + warns.
# 3. MTTCG starvation: 4 vCPU threads on a loaded 4-core host stall the boot
#    with no serial output. Single-threaded TCG by default (MYOS_TCG_SINGLE).
# 4. Silent hangs: watchdog kills the run if the log stops growing 3 min.
#
# Usage: scripts/local-ci.sh [bios|uefi|aarch64|riscv64]   (default bios)
set -euo pipefail
MODE="${1:-bios}"

# OOM protection for everything we spawn (needs root; ignore otherwise).
if [ -w /proc/self/oom_score_adj ]; then
    echo -500 > /proc/self/oom_score_adj 2>/dev/null || true
fi

# Preflight: kill stale QEMU from aborted runs.
pkill -9 -f qemu-system 2>/dev/null || true
sleep 1

# Port guards mirror src/main.rs guestfwd/hostfwd conditions.
ss -tln | grep -q ':2323 ' &&
    echo "WARNING: host :2323 busy -> listen smoke hostfwd will be skipped" >&2
ss -tln | grep -q ':8765 ' &&
    echo "NOTE: host :8765 busy -> guestfwd will be skipped" >&2

cd "$(dirname "$0")/.."
export MYOS_TCG_SINGLE="${MYOS_TCG_SINGLE:-1}"

LOG="${TMPDIR:-/tmp}/localci-$MODE-$(date +%s).log"
nice -n 5 cargo +nightly-2026-07-26 run --quiet -- "$MODE" --ci >"$LOG" 2>&1 &
PID=$!

LAST=0
STALL_SECS=0
while kill -0 "$PID" 2>/dev/null; do
    sleep 20
    SIZE=$(stat -c%s "$LOG" 2>/dev/null || echo 0)
    if [ "$SIZE" = "$LAST" ]; then
        STALL_SECS=$((STALL_SECS + 20))
        if [ "$STALL_SECS" -ge 180 ]; then
            echo "STALLED (no output for 3 min) — killed; log: $LOG" >&2
            tail -5 "$LOG" >&2
            killall -9 qemu-system-x86_64 qemu-system-aarch64 qemu-system-riscv64 2>/dev/null || true
            kill -9 "$PID" 2>/dev/null || true
            exit 124
        fi
    else
        STALL_SECS=0
        LAST=$SIZE
    fi
done
STATUS=0
wait "$PID" || STATUS=$?
tail -5 "$LOG"
echo "log: $LOG"
exit "$STATUS"
