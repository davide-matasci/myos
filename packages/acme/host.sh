#!/usr/bin/env bash
# The host's side of acme's test (test.sh, docs/testing.md): the launcher
# runs `host.sh serve` for the guest's `HOST acme serve` line. It runs the
# test ACME server (acme-server.py) at 10.0.2.2:14000 for the guest, which
# validates http-01 against the guest's web server through QEMU's forward
# of port 5002, until the guest asks it to stop (GET /stop) or ten minutes
# pass; a failed run shows its log.
set -u
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

case "${1:-}" in
serve) ;;
*)
  echo "usage: host.sh serve" >&2
  exit 2
  ;;
esac
if ! command -v python3 >/dev/null 2>&1 || ! command -v openssl >/dev/null 2>&1; then
  echo "boot test: acme serve: python3 and openssl are needed on the host" >&2
  exit 1
fi
log="$(mktemp "${TMPDIR:-/tmp}/myos-acme-server.XXXXXX")"
if python3 "$here/acme-server.py" --listen 0.0.0.0:14000 --url http://10.0.2.2:14000 \
  --http01 127.0.0.1:5002 --timeout 600 2>"$log"; then
  rm -f "$log"
  exit 0
fi
echo "boot test: acme serve: the ACME server failed; its log:" >&2
cat "$log" >&2
rm -f "$log"
exit 1
