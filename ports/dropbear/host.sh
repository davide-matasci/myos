#!/usr/bin/env bash
# The host's side of dropbear's test (test.sh, docs/testing.md): the
# launcher runs `host.sh PORT` for the guest's `HOST dropbear PORT` line.
# Two SSH sessions at once (pubkey auth with the test key) through QEMU's
# port forward, both must exit 0: multi-session accept on dropbear and netd
# (a parked accept plus the listen hold). Each session echoes its tag and
# writes its PATH to /tmp/ssh-ok-<tag>, which the guest test waits for.
set -u
port="${1:?port}"
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

if ! command -v ssh >/dev/null 2>&1; then
  echo "boot test: ssh $port: no ssh client on the host (openssh-client)" >&2
  exit 1
fi
# OpenSSH refuses a world-readable private key: a 0600 copy of the
# committed test key.
dir="$(mktemp -d "${TMPDIR:-/tmp}/myos-dropbear-ssh.XXXXXX")"
trap 'rm -rf "$dir"' EXIT
key="$dir/testkey"
cp "$here/testkey" "$key" && chmod 600 "$key"

# One session: `timeout` so a hung key exchange cannot burn the whole bound
# (ConnectTimeout covers the TCP connect only). The KEX is pinned to
# curve25519: OpenSSH 10 negotiates post-quantum KEX first, which hits a
# dropbear interop bug on aarch64.
one() {
  local tag="$1" out
  out="$(timeout 20 ssh -4 -i "$key" -p "$port" \
    -o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null \
    -o GlobalKnownHostsFile=/dev/null -o BatchMode=yes -o IdentitiesOnly=yes \
    -o PreferredAuthentications=publickey -o KexAlgorithms=curve25519-sha256 \
    -o ConnectTimeout=8 -o ConnectionAttempts=1 \
    "root@127.0.0.1" "echo $tag; echo \"\$PATH\" > /tmp/ssh-ok-$tag" 2>&1)" \
    && [[ "$out" == *"$tag"* ]] && return 0
  echo "$out" > "$dir/err-$tag"
  return 1
}

# Early SYNs before dropbear has armed accept leave half-open sessions on
# slow arches: give it time, and never probe the port. B starts a second
# after A so both are live together while the second SYN usually meets a
# re-armed listener.
sleep 12
deadline=$((SECONDS + 300))
while (( SECONDS < deadline )); do
  one a & pa=$!
  sleep 1
  one b & pb=$!
  if wait "$pa" && wait "$pb"; then
    echo "boot test: ssh $port: two sessions ok" >&2
    exit 0
  fi
  # Failed attempts can leave SynReceived orphans until the handshake-age
  # reclaim (~10 s); flooding starved riscv64.
  sleep 5
done
echo "boot test: ssh $port: gave up ($(cat "$dir"/err-* 2>/dev/null | tail -n 3 | tr '\n' ' '))" >&2
exit 1
