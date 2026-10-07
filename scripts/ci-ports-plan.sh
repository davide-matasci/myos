#!/usr/bin/env bash
# The plan job of ci-ports.yml: ask the registry (manifests only, no
# download) which pieces it lacks at their current input hash, all at once.
# Writes GitHub outputs to $GITHUB_OUTPUT (stdout when unset):
#   ports=<JSON>     the ports matrix (scripts/ports.sh --matrix) reduced to
#                    the ports the registry lacks; `[]` skips the ports job
#   sysroot=, newlib=  true when that toolchain job has to run
# A port job pulls what it depends on and builds a dependency the registry
# lacks itself (ci-build-port.sh), so leaving the hits out is safe; the
# build job pulls every port either way.
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"
chmod +x scripts/*.sh ports/*/*.sh toolchain/*/*.sh 2>/dev/null || true

matrix="$(./scripts/ports.sh --matrix)"
mapfile -t ports < <(python3 -c 'import json, sys
for e in json.load(sys.stdin): print(e["port"])' <<<"$matrix")

./scripts/ci-registry.sh login || true
logs="$(mktemp -d "${RUNNER_TEMP:-${TMPDIR:-/tmp}}/myos-ci-plan.XXXXXX")"
jobs=0
for p in sysroot newlib "${ports[@]}"; do
  ( ./scripts/ci-registry.sh exists "$p" >"$logs/$p" 2>&1 && touch "$logs/$p.hit" ) || true &
  jobs=$((jobs + 1))
  if (( jobs >= 12 )); then
    wait -n || true
    jobs=$((jobs - 1))
  fi
done
wait
misses=()
for p in sysroot newlib "${ports[@]}"; do
  cat "$logs/$p"
  [[ -f "$logs/$p.hit" ]] || misses+=("$p")
done
rm -rf "$logs"

out="${GITHUB_OUTPUT:-/dev/stdout}"
for t in sysroot newlib; do
  if [[ " ${misses[*]-} " == *" $t "* ]]; then
    echo "$t=true" >>"$out"
  else
    echo "$t=false" >>"$out"
  fi
done
echo "ports=$(python3 -c 'import json, sys
misses = set(sys.argv[1:])
print(json.dumps([e for e in json.load(sys.stdin) if e["port"] in misses], separators=(",", ":")))' \
  "${misses[@]+"${misses[@]}"}" <<<"$matrix")" >>"$out"
echo "registry lacks: ${misses[*]:-nothing}"
