#!/usr/bin/env bash
# The CI build job: pull every prebuilt port from GHCR, build what is
# missing, then the kernels and the images (ci-build-kernels.sh).
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"
chmod +x scripts/*.sh ports/*/*.sh toolchain/*/*.sh

# One registry login for the whole job (the pulls below run in parallel).
./scripts/ci-registry.sh login || true
./scripts/ci-registry.sh pull sysroot || true
if compgen -G "target/myos-sysroot-*.tar.zst" > /dev/null; then
  export MYOS_SYSROOT_TARBALL="$(ls target/myos-sysroot-*.tar.zst | head -1)"
fi
./toolchain/std/fetch-sysroot.sh

# Every port with a build script (scripts/ports.sh --build-list all: newlib
# first, then the image ports and the packages, which the build job packs
# too) and the Linux layer's musl pieces. Each is one small OCI artifact, so
# the pulls run several at a time (they wait on the network, not the CPU); a port the registry lacks is built by
# ci-build-kernels.sh and pushed here afterwards. A hit is never pushed
# again: that round trip per port used to cost as much as the pulls.
pieces=()
while read -r name _; do
  [[ "$name" == sysroot ]] || pieces+=("$name")
done < <(./scripts/ports.sh --build-list all)
pieces+=(linux-compat)
logs="$(mktemp -d "${RUNNER_TEMP:-${TMPDIR:-/tmp}}/myos-ci-pull.XXXXXX")"
jobs=0
for p in "${pieces[@]}"; do
  ./scripts/ci-registry.sh pull "$p" >"$logs/$p" 2>&1 || true &
  jobs=$((jobs + 1))
  if (( jobs >= 12 )); then
    wait -n || true
    jobs=$((jobs - 1))
  fi
done
wait
misses=()
for p in "${pieces[@]}"; do
  cat "$logs/$p"
  grep -q "^registry hit " "$logs/$p" || misses+=("$p")
done
rm -rf "$logs"

MYOS_CI_KERNELS_PUSH=0 ./scripts/ci-build-kernels.sh

# The pushes (the kernels when built here, a port the registry lacked) only
# serve later runs: they go on in the background while the job tests, packs
# and uploads, and the job's last step waits for them
# (ci-registry-push-wait.sh). Detached, with no handle on this step's
# output, so the step ends now.
pushes=("${misses[@]+"${misses[@]}"}" kernels)
state="${RUNNER_TEMP:-${TMPDIR:-/tmp}}/myos-ci-push"
rm -rf "$state"
mkdir -p "$state"
setsid nohup bash -c '
  for p in "$@"; do
    ./scripts/ci-registry.sh push "$p" || true
  done
  touch "'"$state"'/done"
' push-bg "${pushes[@]}" </dev/null >"$state/log" 2>&1 &
echo "$!" >"$state/pid"
echo "==> registry pushes in the background: ${pushes[*]}"
