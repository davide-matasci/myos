#!/usr/bin/env bash
# The plan jobs of ci-ports.yml: ask the registry (manifests only, no
# download) which pieces it lacks at their current input hash, in build
# order (ci-registry.sh plan: a port's hash covers its dependencies' output
# hashes, which the registry's answers for them give). Two stages, since a
# port over one the registry lacks has no key until that one is pushed, and
# is often a hit then (its dependency's outputs unchanged):
#   ci-ports-plan.sh         the plan job, before anything is built
#   ci-ports-plan.sh after   the replan job, once the first stage has pushed
# Writes GitHub outputs to $GITHUB_OUTPUT (stdout when unset). The first
# stage:
#   ports=<JSON>     the ports matrix (scripts/ports.sh --matrix) reduced to
#                    the ports the registry lacks under a key it can
#                    compute, and the arches of a port built an arch at a
#                    time (PORT_ARCH_JOBS) whatever their key: `[]` skips
#                    the ports job
#   replan=          true when other ports wait for the second stage
#   sysroot=, newlib=  true when that toolchain job has to run
# The second:
#   ports_after=<JSON>  the ports the registry still lacks (the ports-after
#                    job)
# A port job pulls what it depends on and builds a dependency the registry
# lacks itself (ci-build-port.sh), so the second stage needs no third: a
# port over one that stage builds gets a job in it too. Not one built an
# arch at a time, though, whose dependents (surf) could not build it in
# their job: its arches are all in the first stage, where each builds a
# missing dependency itself. The build job pulls every port either way.
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"
chmod +x scripts/*.sh ports/*/*.sh toolchain/*/*.sh 2>/dev/null || true

stage="${1:-first}"
matrix="$(./scripts/ports.sh --matrix)"
mapfile -t ports < <(python3 -c 'import json, sys
for e in json.load(sys.stdin): print(e["port"])' <<<"$matrix")

./scripts/ci-registry.sh login || true
# `<port> missing|undefined` per line.
misses="$(./scripts/ci-registry.sh plan sysroot newlib "${ports[@]}")"

out="${GITHUB_OUTPUT:-/dev/stdout}"
python3 -c 'import json, sys
out, stage, plan = sys.argv[1:4]
state = dict(line.split() for line in plan.splitlines() if line.strip())
lacking = [e for e in json.load(sys.stdin) if e["port"] in state]
with open(out, "a") as f:
    if stage == "after":
        f.write("ports_after=" + json.dumps(lacking, separators=(",", ":")) + "\n")
        sys.exit(0)
    # Now: a key the registry has nothing under, or an arch of a port
    # built an arch at a time ("name:arch").
    now = [e for e in lacking if state[e["port"]] == "missing" or ":" in e["port"]]
    f.write("ports=" + json.dumps(now, separators=(",", ":")) + "\n")
    f.write("replan=" + ("true" if len(now) < len(lacking) else "false") + "\n")
    for t in ("sysroot", "newlib"):
        f.write(t + "=" + ("true" if t in state else "false") + "\n")' \
  "$out" "$stage" "$misses" <<<"$matrix"
echo "registry lacks: $(tr '\n' ' ' <<<"$misses" | sed 's/ $//;s/^$/nothing/')"
