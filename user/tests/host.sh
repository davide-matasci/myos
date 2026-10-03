#!/usr/bin/env bash
# The host's side of the core tests (docs/testing.md): the launcher runs
# `host.sh <what> <args>` for a `HOST tests <what> <args>` line from the
# guest, in the background; the guest test decides from what it sees.
#
#   tcp-ping PORT   the listen/accept smoke's peer: connect to the guest's
#                   listener through QEMU's port forward, send "ping",
#                   expect "pong". Retried until the listener is up; the
#                   guest test bounds its own wait.
set -u

tcp_ping() {
  local port="$1" reply deadline=$((SECONDS + 180))
  while (( SECONDS < deadline )); do
    if exec 3<>"/dev/tcp/127.0.0.1/$port" 2>/dev/null; then
      printf ping >&3
      reply="$(timeout 15 head -c 5 <&3 2>/dev/null || true)"
      exec 3>&-
      if [[ "$reply" == $'pong\n' ]]; then
        echo "boot test: tcp-ping $port: pong" >&2
        return 0
      fi
    fi
    sleep 0.25
  done
  echo "boot test: tcp-ping $port: no pong within the bound" >&2
  return 1
}

case "${1:-}" in
  tcp-ping) tcp_ping "${2:?port}" ;;
  *) echo "host.sh: unknown request: $*" >&2; exit 2 ;;
esac
