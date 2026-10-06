#!/usr/bin/env bash
# The CI build job's last step: wait for the registry pushes that
# ci-build-pull-and-kernels.sh left running in the background and print
# their log. A push failure never fails the job (the next run rebuilds).
set -uo pipefail
state="${RUNNER_TEMP:-${TMPDIR:-/tmp}}/myos-ci-push"
[[ -f "$state/pid" ]] || { echo "no background registry pushes"; exit 0; }
pid="$(cat "$state/pid")"
while [[ ! -f "$state/done" ]] && kill -0 "$pid" 2>/dev/null; do
  sleep 1
done
cat "$state/log"
[[ -f "$state/done" ]] || echo "::warning::background registry pushes stopped before finishing"
exit 0
