#!/usr/bin/env bash
# The plan job of ci-ports.yml: ask the registry (manifests only, no
# download) which pieces it lacks at their current input hash, in build
# order (ci-registry.sh plan: a port's hash covers its dependencies' output
# hashes, which the registry's answers for them give). Writes GitHub
# outputs to $GITHUB_OUTPUT (stdout when unset):
#   ports=<JSON>     the ports matrix (scripts/ports.sh --matrix) reduced to
#                    the ports the registry lacks; `[]` skips the ports job
#   ports_after=<JSON>  of those, the ones that depend on a port built an
#                    arch at a time (PORT_ARCH_JOBS) that the registry lacks
#                    too: they wait for its arch jobs (the ports-after job)
#                    instead of building all of it in their own
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
mapfile -t misses < <(./scripts/ci-registry.sh plan sysroot newlib "${ports[@]}")

out="${GITHUB_OUTPUT:-/dev/stdout}"
for t in sysroot newlib; do
  if [[ " ${misses[*]-} " == *" $t "* ]]; then
    echo "$t=true" >>"$out"
  else
    echo "$t=false" >>"$out"
  fi
done
python3 -c 'import json, sys
misses = set(sys.argv[2:])
lacking = [e for e in json.load(sys.stdin) if e["port"] in misses]
# The ports built an arch at a time ("name:arch" entries) that are missing.
split = {e["port"].split(":")[0] for e in lacking if ":" in e["port"]}
after = [e for e in lacking if split & set(e["deps"].split())]
now = [e for e in lacking if e not in after]
with open(sys.argv[1], "a") as f:
    f.write("ports=" + json.dumps(now, separators=(",", ":")) + "\n")
    f.write("ports_after=" + json.dumps(after, separators=(",", ":")) + "\n")' \
  "$out" "${misses[@]+"${misses[@]}"}" <<<"$matrix"
echo "registry lacks: ${misses[*]:-nothing}"
