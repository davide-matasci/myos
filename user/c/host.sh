#!/usr/bin/env bash
# The host's side of the C smokes' tests (docs/testing.md): the launcher runs
# `host.sh <what> <args>` for a `HOST c-smokes <what> <args>` line from the
# guest, in the background; the guest test decides from what it sees.
#
#   tcp-ping PORT   the listen/accept smoke's peer: connect to the guest's
#                   listener through QEMU's port forward, send "ping",
#                   expect "pong" and then the smoke's numbered lines up to
#                   its close; connect again to say "good" if every byte
#                   arrived ("bad <bytes>" if not). Retried until the
#                   listener is up; the guest test bounds its own wait.
#   sendkey KEYS... type each of KEYS (QEMU's names, `shift-a`), in order,
#                   on the guest's keyboard through the QEMU monitor (the
#                   launcher's socket, $MYOS_QEMU_MONITOR). One request per
#                   key would not keep the order: requests run in parallel.
#   keyburst N KEY  type KEY N times, 10 ms apart: a burst of interrupts
#                   for the keyboard's readers to drain together.
#   mouse [ps2]     move the guest's usb-mouse 10 right and 5 up, then
#                   click its left button (mouse_smoke.c), through the
#                   monitor; with `ps2`, QEMU's PS/2 mouse instead (the
#                   monitor's `mouse_set` makes it the one that moves, and
#                   gives the usb-mouse back after).
set -u

# The smoke's bulk: BULK_LINES lines "%06d\n" (tcp_listen_smoke.c).
BULK_LINES=40000

tcp_ping() {
  local port="$1" got verdict deadline=$((SECONDS + 180))
  got="$(mktemp)"
  while (( SECONDS < deadline )); do
    # The group keeps the connect error quiet without redirecting the
    # script's stderr for good, as `exec ... 2>/dev/null` would.
    if { exec 3<>"/dev/tcp/127.0.0.1/$port"; } 2>/dev/null; then
      printf ping >&3
      # Everything up to the smoke's close: "pong\n", then the lines.
      timeout 120 cat <&3 > "$got" 2>/dev/null || true
      exec 3>&-
      if [[ "$(head -c 5 "$got")" == pong ]]; then
        local diff
        diff="$(tail -c +6 "$got" | cmp - <(seq -f %06g 0 $((BULK_LINES - 1))) 2>&1)"
        if [[ -z "$diff" ]]; then
          verdict=good
        else
          # The byte count and where the stream first goes wrong.
          verdict="bad $(($(wc -c < "$got") - 5)): ${diff#*: }"
        fi
        echo "boot test: tcp-ping $port: pong, bulk $verdict" >&2
        if { exec 3<>"/dev/tcp/127.0.0.1/$port"; } 2>/dev/null; then
          printf '%s\n' "$verdict" >&3
          sleep 1
          exec 3>&-
        fi
        rm -f "$got"
        [[ "$verdict" == good ]]
        return
      fi
    fi
    sleep 0.25
  done
  rm -f "$got"
  echo "boot test: tcp-ping $port: no pong within the bound" >&2
  return 1
}

sendkey() {
  python3 - "${MYOS_QEMU_MONITOR:?no QEMU monitor}" "$@" <<'PY'
import socket, sys, time
s = socket.socket(socket.AF_UNIX)
s.connect(sys.argv[1])
s.settimeout(5)
s.recv(4096)  # the banner and prompt
for key in sys.argv[2:]:
    s.sendall(f"sendkey {key}\n".encode())
    time.sleep(0.5)
time.sleep(1)
s.close()
PY
  echo "boot test: sendkey $*" >&2
}

keyburst() {
  python3 - "${MYOS_QEMU_MONITOR:?no QEMU monitor}" "$1" "$2" <<'PY'
import socket, sys, time
s = socket.socket(socket.AF_UNIX)
s.connect(sys.argv[1])
s.settimeout(5)
s.recv(4096)  # the banner and prompt
for _ in range(int(sys.argv[2])):
    s.sendall(f"sendkey {sys.argv[3]}\n".encode())
    time.sleep(0.01)
time.sleep(1)
s.close()
PY
  echo "boot test: keyburst $1 $2" >&2
}

mouse() {
  python3 - "${MYOS_QEMU_MONITOR:?no QEMU monitor}" "${1:-usb}" <<'PY'
import re, socket, sys, time
s = socket.socket(socket.AF_UNIX)
s.connect(sys.argv[1])
s.settimeout(5)
s.recv(4096)  # the banner and prompt

def run(cmd):
    s.sendall(f"{cmd}\n".encode())
    time.sleep(0.5)
    try:
        return s.recv(65536).decode(errors="replace")
    except Exception:
        return ""

# "  Mouse #2: QEMU PS/2 Mouse", the active one starred.
mice = dict((name, n) for n, name in re.findall(r"Mouse #(\d+): (QEMU [^\r\n]+)", run("info mice")))
ps2 = sys.argv[2] == "ps2"
if ps2:
    run(f"mouse_set {mice.get('QEMU PS/2 Mouse', '')}")
for cmd in ("mouse_move 10 -5", "mouse_button 1", "mouse_button 0"):
    run(cmd)
if ps2 and "QEMU HID Mouse" in mice:
    run(f"mouse_set {mice['QEMU HID Mouse']}")
time.sleep(1)
s.close()
PY
  echo "boot test: mouse ${1:-usb}" >&2
}

case "${1:-}" in
  tcp-ping) tcp_ping "${2:?port}" ;;
  sendkey) shift; sendkey "${1:?keys}" "${@:2}" ;;
  keyburst) keyburst "${2:?count}" "${3:?key}" ;;
  mouse) mouse "${2:-usb}" ;;
  *) echo "host.sh: unknown request: $*" >&2; exit 2 ;;
esac
