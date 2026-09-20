#!/bin/sh
# Full-boot wait_ci curl smoke. One retry: UEFI/slirp sometimes returns
# curl (7) connect-fail right after a successful `http` HTTPS GET (same SHA
# green on bios/aarch64/riscv64). Keep this script short so the interactive
# echo `$ sh /lib/ci-curl-smoke.sh` stays well under 160 cols.
URL=https://example.com/
OUT=/tmp/curl-ex.html
if ! curl -fsS --connect-timeout 30 --max-time 90 -o "$OUT" "$URL"; then
	sleep 2
	curl -fsS --connect-timeout 30 --max-time 90 -o "$OUT" "$URL" || true
fi
cat "$OUT"
