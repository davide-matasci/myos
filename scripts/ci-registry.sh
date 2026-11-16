#!/bin/sh
# Pull/push myos userspace *outputs* as GHCR OCI artifacts (oras).
#
# Tags are the stamp hashes from myos-c-userspace-lib.sh / toolchain/std/lib.sh:
# a port's input hash, which covers the *output hashes* of the ports it
# builds against (myos_dep_hash, docs/ports.md "Cache keys"). A push
# records a port's output hash in its outputs stamp (packed with it) and as
# an annotation of the artifact's manifest; a pull and the plan read the
# annotation into target/.myos-dep-hash/, where the dependents' hashes find
# it without a download.
# Cache ELFs, newlib prefixes, stamps — never *-src or *-myos-build trees.
# skip-if-fresh (myos_*_is_current) stays the local truth after a pull.
#
# Usage:
#   ./scripts/ci-registry.sh pull PORT
#   ./scripts/ci-registry.sh push PORT
#   ./scripts/ci-registry.sh current PORT   exit 0 when PORT's outputs are current
#   ./scripts/ci-registry.sh exists PORT    exit 0 when the registry has PORT at its hash
#   ./scripts/ci-registry.sh plan PORT...   the ports the registry lacks, in order, each with `missing` or `undefined` (the CI plan)
#   ./scripts/ci-registry.sh outputs PORT   PORT's output hash (recorded from its files when current)
# PORT is a port with a descriptor and a build script (`scripts/ports.sh
# --list`: its stamp and PORT_OUTPUTS are cached, keyed by its
# myos_<name>_version_hash), or one of the pieces cached the same way:
# sysroot newlib linux-compat kernels. "all" does sysroot + newlib first.
# A port built an arch at a time (PORT_ARCH_JOBS, docs/ports.md) is cached
# as a piece per arch, `<name>:<arch>`; the port's name means all three.
#
# Env:
#   GITHUB_TOKEN              required for private GHCR; pull is anonymous only when unset
#   GITHUB_ACTOR              oras login user (fallback: GITHUB_REPOSITORY_OWNER)
#   GITHUB_REPOSITORY         owner/repo (default davide-matasci/myos)
#   MYOS_CI_REGISTRY_PUSH     false/0 skips push (fork PRs)
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
# shellcheck source=scripts/myos-c-userspace-lib.sh
. "$ROOT/scripts/myos-c-userspace-lib.sh"
# shellcheck source=toolchain/std/lib.sh
. "$ROOT/toolchain/std/lib.sh"
# shellcheck source=scripts/ports.sh
. "$ROOT/scripts/ports.sh"

ORAS_VERSION="1.3.3"
ORAS_LINUX_AMD64_SHA256="9ce999f8d2de03fc03968b29d743077a58783e545e5eaa53917ca177352d0e59"
ORAS_LINUX_ARM64_SHA256="ac7156f93a21e903f7ad606c792f3560f17e0cd0e36365634701b1e7cc4e4eca"
ORAS_ARTIFACT_TYPE="application/vnd.myos.ci.port.v1"
ORAS_LAYER_TYPE="application/vnd.myos.ci.port.layer.v1.tar+zst"
# The manifest annotation carrying a port's output hash.
ORAS_OUTPUTS_ANNOTATION="org.myos.ci.outputs"

# Every port with a build script in build order (the toolchains first:
# sysroot and newlib, which the others pull; image and package ports), then
# the Linux layer's musl pieces and the kernels.
ALL_PORTS="$(myos_port_build_order all | tr '\n' ' ')linux-compat kernels"

usage() {
  echo "usage: $0 pull|push|current|exists|outputs PORT | plan PORT... | login" >&2
  echo "  PORT: $ALL_PORTS all" >&2
  exit 2
}

# Dispatch to the hash / freshness functions of myos-c-userspace-lib.sh and
# toolchain/std/lib.sh: a port NAME has myos_<name>_version_hash and
# myos_<name>_is_current (dashes as underscores; the sysroot's are in
# toolchain/std/lib.sh).
port_fn() (
  fn="myos_$(printf '%s' "$1" | tr - _)_$2"
  if ! command -v "$fn" >/dev/null 2>&1; then
    echo "error: no function $fn for port $1 (scripts/myos-c-userspace-lib.sh)" >&2
    return 2
  fi
  echo "$fn"
)

# The pieces PORT stands for: itself, or `PORT:<arch>` per arch for a port
# built an arch at a time (a piece stands for itself).
port_pieces() {
  case "$1" in
    *:*|sysroot|newlib|linux-compat|kernels) echo "$1" ;;
    *) myos_port_jobs "$1" ;;
  esac
}

# A piece's port and arch: `NAME:ARCH` sets MYOS_<NAME>_ARCHES=ARCH for the
# port's functions (its build script and is_current read it).
piece_env() (
  name="${1%%:*}"
  name="$(printf '%s' "$name" | tr '[:lower:]-' '[:upper:]_')"
  printf 'MYOS_%s_ARCHES=%s' "$name" "${1#*:}"
)

# A piece's input hash; 1 (nothing printed) when it is undefined: the
# outputs of a port it builds against are neither built here nor in the
# registry (myos_hash's `unknown-...`).
port_hash() (
  case "$1" in
    *:*) port_hash "${1%%:*}" ;;
    kernels) "$ROOT/scripts/ci-build-kernels.sh" --print-hash | tr -d '
' ;;
    *)
      fn="$(port_fn "$1" version_hash)" || return 2
      h="$("$fn")"
      case "$h" in
        ''|unknown-*) return 1 ;;
      esac
      printf '%s' "$h"
      ;;
  esac
)

# The pieces with an output hash: the ports (the sysroot, the kernels and
# the Linux layer are keyed by their sources alone; nothing builds against
# them through myos_dep_hash).
has_outputs_key() {
  case "$1" in
    sysroot|kernels|linux-compat) return 1 ;;
    *) return 0 ;;
  esac
}

# The output hash annotated on the manifest JSON on stdin, or 1 (an
# artifact without one, or no manifest at all).
annotation_outputs() {
  python3 -c 'import json, sys
try:
    h = json.load(sys.stdin).get("annotations", {}).get(sys.argv[1], "")
except ValueError:
    h = ""
if not h: sys.exit(1)
print(h)' "$ORAS_OUTPUTS_ANNOTATION"
}

# Keep the registry's output hash of PIECE at input hash HASH where the
# dependents' hashes look first (myos_piece_output_hash).
cache_outputs() (
  rec="$(myos_dep_hash_cache "$1")"
  mkdir -p "$(dirname "$rec")"
  printf '%s %s\n' "$2" "$3" >"$rec.tmp.$$"
  mv -f "$rec.tmp.$$" "$rec"
)

# myos_dep_hash's way to the registry (scripts/myos-c-userspace-lib.sh):
# the output hash of PIECE at input hash HASH, from the annotation of its
# manifest (no download); nothing, and 1, when the registry lacks it.
myos_dep_hash_hook() (
  has_outputs_key "$1" || return 1
  ref="$(registry_ref "$1" "$2")"
  ensure_oras >&2
  if [ -n "${GITHUB_TOKEN:-${GH_TOKEN:-}}" ]; then
    oras_login >/dev/null 2>&1 || true
  fi
  json="$(oras manifest fetch "$ref" 2>/dev/null)" || return 1
  printf '%s\n' "$json" | annotation_outputs
)

port_is_current() (
  case "$1" in
    *:*) (export "$(piece_env "$1")"; port_is_current "${1%%:*}") ;;
    kernels) "$ROOT/scripts/ci-build-kernels.sh" --is-current ;;
    *) fn="$(port_fn "$1" is_current)" || return 2; "$fn" ;;
  esac
)

# Print repo-relative paths to pack. Directories are included recursively.
# Never lists *-src / *-myos-build / object trees: a port's descriptor names
# its outputs (PORT_OUTPUTS, scripts/ports.sh --outputs; the sysroot's is
# the slim pack: stamp, manifest, precompiled rlibs and target specs, never
# the patched library/ source tree under rustlib/src).
port_members() (
  port="$1"
  case "$port" in
    linux-compat)
      # The per-arch output dirs only: never the musl prefixes / sources /
      # compiler-rt tree next to them under target/linux-compat.
      echo target/.myos-linux-compat-version
      for arch in x86_64 aarch64 riscv64; do
        echo "target/linux-smoke-${arch}-linux-musl"
        echo "target/linux-compat/${arch}"
      done
      ;;
    kernels)
      "$ROOT/scripts/ci-build-kernels.sh" --print-members
      ;;
    *:*) myos_port_outputs "${port%%:*}" "${port#*:}" ;;
    *) myos_port_outputs "$port" ;;
  esac
)

# POSIX sh has no ${var,,}; use tr.
lower() {
  printf '%s' "$1" | tr '[:upper:]' '[:lower:]'
}

repo_lower() {
  lower "${GITHUB_REPOSITORY:-davide-matasci/myos}"
}

registry_ref() {
  # GHCR repository names must be lowercase; stamp hashes may contain hex.
  # A piece's `:` is a `-` (ci-webkitgtk-x86_64).
  printf 'ghcr.io/%s/ci-%s:%s' "$(repo_lower)" "$(lower "$1" | tr : -)" "$2"
}

package_name() (
  repo="$(repo_lower)"
  printf '%s/ci-%s' "${repo#*/}" "$(lower "$1" | tr : -)"
)

ensure_oras() {
  _eo_bindir="${MYOS_ORAS_BINDIR:-$ROOT/target/.oras-bin}"
  export PATH="$_eo_bindir:$PATH"
  if [ -x "$_eo_bindir/oras" ]; then
    return 0
  fi
  _eo_uname_s="$(uname -s)"
  _eo_uname_m="$(uname -m)"
  case "$_eo_uname_s" in
    Linux) _eo_os=linux ;;
    *) echo "error: install oras (https://github.com/oras-project/oras/releases)" >&2; return 1 ;;
  esac
  case "$_eo_uname_m" in
    x86_64|amd64) _eo_arch=amd64; _eo_sha="$ORAS_LINUX_AMD64_SHA256" ;;
    aarch64|arm64) _eo_arch=arm64; _eo_sha="$ORAS_LINUX_ARM64_SHA256" ;;
    *) echo "error: unsupported arch $_eo_uname_m for pinned oras" >&2; return 1 ;;
  esac
  _eo_url="https://github.com/oras-project/oras/releases/download/v${ORAS_VERSION}/oras_${ORAS_VERSION}_${_eo_os}_${_eo_arch}.tar.gz"
  mkdir -p "$_eo_bindir"
  _eo_tgz="$(mktemp "${TMPDIR:-/tmp}/oras.XXXXXX.tar.gz")"
  # Retried on any error, backing off for about a minute: GitHub's release
  # host answered 504 for over 20 s and left a job without oras for its
  # first pull (newlib's), which no later pull redid.
  curl -fsSL --retry 6 --retry-all-errors "$_eo_url" -o "$_eo_tgz"
  echo "${_eo_sha}  ${_eo_tgz}" | sha256sum -c - >/dev/null
  tar -xzf "$_eo_tgz" -C "$_eo_bindir" oras
  rm -f "$_eo_tgz"
  chmod +x "$_eo_bindir/oras"
}

# One oras login per job: `login` writes the marker, the pulls and pushes
# that follow (in parallel, from ci-build-pull-and-kernels.sh) skip theirs.
login_marker() (
  base="${RUNNER_TEMP:-${TMPDIR:-/tmp}}"
  printf '%s' "${base%/}/myos-ci-registry-login-ok"
)

oras_login() (
  if [ -f "$(login_marker)" ]; then
    return 0
  fi
  token="${GITHUB_TOKEN:-${GH_TOKEN:-}}"
  user="${GITHUB_ACTOR:-${GITHUB_REPOSITORY_OWNER:-${GITHUB_REPOSITORY:-davide-matasci/myos}}}"
  user="${user%%/*}"
  user="$(lower "$user")"
  if [ -z "$token" ] || [ -z "$user" ]; then
    echo "registry login failed: missing GITHUB_TOKEN or username"
    echo "registry login failed: missing GITHUB_TOKEN or username" >&2
    return 1
  fi
  err="$(mktemp "${TMPDIR:-/tmp}/oras-login.XXXXXX")"
  # stdin password (oras login -u USER --password-stdin); do not hide failures.
  if ! printf '%s' "$token" | oras login ghcr.io -u "$user" --password-stdin >"$err" 2>&1; then
    echo "registry login failed (user=${user})"
    echo "registry login failed (user=${user})" >&2
    cat "$err"
    cat "$err" >&2
    rm -f "$err"
    return 1
  fi
  rm -f "$err"
  touch "$(login_marker)"
)

can_push() {
  case "${MYOS_CI_REGISTRY_PUSH:-}" in
    0|false|FALSE|no|NO) return 1 ;;
  esac
  [ -n "${GITHUB_TOKEN:-${GH_TOKEN:-}}" ]
}

# Manifests record absolute ELF paths; rewrite to this checkout so is_current holds.
rewrite_manifest_paths() (
  port="$1"
  case "$port" in
    sbase|ubase) ;;
    *) return 0 ;;
  esac
  for f in "$ROOT/target/${port}-manifest-"*.txt; do
    [ -e "$f" ] || continue
    tmp="$(mktemp)"
    while IFS= read -r line || [ -n "$line" ]; do
      [ -n "$line" ] || continue
      case "$line" in
        *:*) ;;
        *)
          printf '%s\n' "$line"
          continue
          ;;
      esac
      name="${line%%:*}"
      path="${line#*:}"
      case "$path" in
        *"/target/"*)
          rel="target/${path#*/target/}"
          path="$ROOT/$rel"
          ;;
        target/*)
          path="$ROOT/$path"
          ;;
        /*)
          if [ ! -f "$path" ]; then
            path="$ROOT/target/$(basename "$path")"
          fi
          ;;
      esac
      printf '%s:%s\n' "$name" "$path"
    done <"$f" >"$tmp"
    mv "$tmp" "$f"
  done
)

existing_members() (
  while IFS= read -r rel; do
    [ -n "$rel" ] || continue
    if [ -e "$ROOT/$rel" ]; then
      printf '%s\n' "$rel"
    fi
  done <<EOF
$(port_members "$1" | awk 'NF && !seen[$0]++')
EOF
)

try_public_package() (
  port="$1"
  token="${GITHUB_TOKEN:-${GH_TOKEN:-}}"
  [ -n "$token" ] || return 0
  owner="${GITHUB_REPOSITORY_OWNER:-${GITHUB_REPOSITORY:-davide-matasci/myos}}"; owner="${owner%%/*}"
  pkg="$(package_name "$port")"
  enc="$(printf '%s\n' "$pkg" | sed 's|/|%2F|g')"
  for url in \
    "https://api.github.com/user/packages/container/${enc}/visibility" \
    "https://api.github.com/orgs/${owner}/packages/container/${enc}/visibility"
  do
    curl -fsS -o /dev/null -X PUT \
      -H "Authorization: Bearer ${token}" \
      -H "Accept: application/vnd.github+json" \
      -H "X-GitHub-Api-Version: 2022-11-28" \
      "$url" \
      -d '{"visibility":"public"}' >/dev/null 2>&1 || true
  done
)

force_replace_marker() (
  # Prefer runner temp /tmp — never under target/ (builds may wipe it).
  base="${RUNNER_TEMP:-${TMPDIR:-/tmp}}"
  printf '%s' "${base%/}/myos-ci-registry-replace-$1"
)

mark_force_replace() {
  printf '%s\n' "$2" >"$(force_replace_marker "$1")"
}

clear_force_replace() {
  rm -f "$(force_replace_marker "$1")"
}

needs_force_replace() {
  # Presence alone: pull may have stored a different hash if stamps used to be
  # non-deterministic across the same job.
  [ -f "$(force_replace_marker "$1")" ]
}

# 0 when the registry has PORT at its current input hash (manifest only, no
# download): what the CI plan asks before running a toolchain job.
cmd_exists() (
  port="$1"
  if ! hash="$(port_hash "$port")"; then
    echo "registry lacks ${port}: its key is undefined (a dependency's outputs are not in the registry)"
    return 1
  fi
  ref="$(registry_ref "$port" "$hash")"
  ensure_oras
  if [ -n "${GITHUB_TOKEN:-${GH_TOKEN:-}}" ]; then
    oras_login || true
  fi
  if oras manifest fetch "$ref" >/dev/null 2>&1; then
    echo "registry has ${port} ${hash}"
    return 0
  fi
  echo "registry lacks ${port} ${hash}"
  return 1
)

cmd_login() {
  ensure_oras
  oras_login
}

cmd_pull() (
  port="$1"
  if ! hash="$(port_hash "$port")"; then
    echo "registry miss ${port}: its key is undefined (a dependency's outputs are not built here nor in the registry)"
    return 0
  fi
  ref="$(registry_ref "$port" "$hash")"
  clear_force_replace "$port"
  ensure_oras
  # Token present: require login so private GHCR packages are visible.
  # Anonymous fetch only when no token (public packages / empty GHCR).
  if [ -n "${GITHUB_TOKEN:-${GH_TOKEN:-}}" ]; then
    if ! oras_login; then
      mark_force_replace "$port" "$hash"
      echo "registry miss ${port}: login failed ${hash}"
      return 0
    fi
  fi
  if ! oras manifest fetch "$ref" >/dev/null 2>&1; then
    mark_force_replace "$port" "$hash"
    echo "registry miss ${port}: no manifest ${hash}"
    return 0
  fi
  tmp="$(mktemp -d "${TMPDIR:-/tmp}/myos-ci-reg.XXXXXX")"
  if ! oras pull "$ref" -o "$tmp" >/dev/null 2>&1; then
    rm -rf "$tmp"
    mark_force_replace "$port" "$hash"
    echo "registry miss ${port}: pull failed ${hash}"
    return 0
  fi
  tarball=""
  for f in "$tmp"/*.tar.zst "$tmp"/*.tar.zstd; do
    [ -f "$f" ] || continue
    tarball="$f"
    break
  done
  if [ -z "$tarball" ]; then
    rm -rf "$tmp"
    mark_force_replace "$port" "$hash"
    echo "registry miss ${port}: no tarball ${hash}"
    return 0
  fi
  mkdir -p "$ROOT/target"
  if ! tar -C "$ROOT" --zstd --no-same-owner -xf "$tarball" 2>/dev/null; then
    rm -rf "$tmp"
    mark_force_replace "$port" "$hash"
    echo "registry miss ${port}: extract failed ${hash}"
    return 0
  fi
  rewrite_manifest_paths "$port"
  rm -rf "$tmp"
  if port_is_current "$port"; then
    clear_force_replace "$port"
    # The output hash, as the registry has it (the artifact's own stamp
    # says the same; one without is recorded from its files).
    if has_outputs_key "$port"; then
      if outputs="$(oras manifest fetch "$ref" 2>/dev/null | annotation_outputs)" && [ -n "$outputs" ]; then
        cache_outputs "$port" "$hash" "$outputs"
      fi
      [ -f "$(myos_outputs_stamp "$port")" ] || myos_outputs_record "$port" >/dev/null
    fi
    echo "registry hit ${port} ${hash}"
  else
    mark_force_replace "$port" "$hash"
    echo "registry miss ${port}: not current after extract ${hash}"
    # Name what the package lacks: the port is rebuilt (or completed) and
    # pushed in its place, and the log says why.
    while IFS= read -r f; do
      { [ -z "$f" ] || [ -e "$ROOT/$f" ]; } || echo "registry pull ${port}: missing member ${f}"
    done <<EOF
$(port_members "$port" | awk 'NF && !seen[$0]++')
EOF
  fi
)

# The output hash's annotation is this function's argument list (`set --`).
cmd_push() (
  port="$1"
  outputs=""
  set --
  if ! can_push; then
    echo "registry skip push (disabled)"
    return 0
  fi
  if ! port_is_current "$port"; then
    echo "registry skip push ${port}: not current"
    return 0
  fi
  if ! hash="$(port_hash "$port")"; then
    echo "registry skip push ${port}: its key is undefined"
    return 0
  fi
  ref="$(registry_ref "$port" "$hash")"
  ensure_oras
  if ! oras_login; then
    echo "registry push failed ${port}: login failed"
    echo "registry push failed ${port}: login failed" >&2
    return 1
  fi
  if oras manifest fetch "$ref" >/dev/null 2>&1; then
    if needs_force_replace "$port" "$hash"; then
      echo "registry replace ${port}: remote present but pull marked replace ${hash}"
      # Drop the poisoned tag so the subsequent push publishes a fresh package.
      oras manifest delete "$ref" --force >/dev/null 2>&1 || true
    else
      echo "registry skip push ${port}: already present ${hash}"
      return 0
    fi
  fi
  # The output hash, in the outputs stamp (packed below) and on the manifest.
  if has_outputs_key "$port"; then
    outputs="$(myos_outputs_record "$port")"
    cache_outputs "$port" "$hash" "$outputs"
    set -- --annotation "${ORAS_OUTPUTS_ANNOTATION}=${outputs}"
  fi
  tmp="$(mktemp -d "${TMPDIR:-/tmp}/myos-ci-reg.XXXXXX")"
  list="$tmp/members.txt"
  existing_members "$port" >"$list"
  if [ ! -s "$list" ]; then
    rm -rf "$tmp"
    echo "registry skip push ${port}: nothing to pack"
    return 0
  fi
  # zstd on every core: the kernels package (the disk images) is large;
  # the images are sparse (their partitions mostly unwritten), kept so.
  tar -C "$ROOT" --sparse -I 'zstd -T0' -cf "$tmp/${port}.tar.zst" -T "$list"
  # The jobs that found the same dependency missing build it in parallel,
  # and the pack takes long enough (a minute for a large port) for every
  # one of them to have seen the tag empty above: ask again.
  if oras manifest fetch "$ref" >/dev/null 2>&1; then
    rm -rf "$tmp"
    echo "registry skip push ${port}: already present ${hash} (pushed meanwhile)"
    return 0
  fi
  # oras push rejects absolute file paths; push from $tmp with a relative name.
  set +e
  (
    cd "$tmp" && oras push "$ref" \
      --artifact-type "$ORAS_ARTIFACT_TYPE" "$@" \
      "${port}.tar.zst:${ORAS_LAYER_TYPE}" >/dev/null
  ) 2>"$tmp/oras.err"
  status=$?
  set -e
  if [ "$status" -ne 0 ]; then
    echo "registry push failed ${port}"
    echo "registry push failed ${port}" >&2
    if [ -s "$tmp/oras.err" ]; then
      cat "$tmp/oras.err"
      cat "$tmp/oras.err" >&2
    fi
    rm -rf "$tmp"
    return 1
  fi
  try_public_package "$port"
  clear_force_replace "$port"
  rm -rf "$tmp"
  # A registry has no "put if absent": a push between the check and ours
  # was overwritten, and one after ours overwrote it. The tag's record now
  # says whose build stays; when it is not this one, the caller is told the
  # port was already there and takes that build before keying anything on
  # its own (ci-build-port.sh). What is left of the race is the time
  # between two requests, not the pack's.
  if [ -n "${outputs:-}" ]; then
    if theirs="$(oras manifest fetch "$ref" 2>/dev/null | annotation_outputs)" && [ "$theirs" != "$outputs" ]; then
      cache_outputs "$port" "$hash" "$theirs"
      echo "registry skip push ${port}: already present ${hash} (another build's outputs replaced this push)"
      return 0
    fi
  fi
  echo "registry push ${port} ${hash}"
)

# The CI plan: of the pieces of the PORTs given, in build order, the ones
# the registry lacks at their input hash (manifests only, no download), one
# `<port> <state>` per line. A hit's output hash goes to
# target/.myos-dep-hash/, where the hashes of the ports after it find it.
# The state is `missing` for a port with a key the registry has nothing
# under, and `undefined` for one whose dependency the registry lacks: it has
# no key until that dependency is pushed, and may be a hit then, the
# dependency's outputs unchanged (the plan's second stage asks again,
# scripts/ci-ports-plan.sh).
cmd_plan() (
  misses=""
  undefined=" "
  ensure_oras >&2
  if [ -n "${GITHUB_TOKEN:-${GH_TOKEN:-}}" ]; then
    oras_login >&2 || true
  fi
  for port in "$@"; do
    for piece in $(port_pieces "$port"); do
      if ! hash="$(port_hash "$piece")"; then
        echo "registry lacks ${piece}: its key is undefined (a dependency's outputs are not in the registry)" >&2
        misses="$misses $port"
        undefined="$undefined$port "
        continue
      fi
      ref="$(registry_ref "$piece" "$hash")"
      if json="$(oras manifest fetch "$ref" 2>/dev/null)"; then
        echo "registry has ${piece} ${hash}" >&2
        if has_outputs_key "$piece" && outputs="$(printf '%s\n' "$json" | annotation_outputs)" && [ -n "$outputs" ]; then
          cache_outputs "$piece" "$hash" "$outputs"
        fi
      else
        echo "registry lacks ${piece} ${hash}" >&2
        misses="$misses $port"
      fi
    done
  done
  # shellcheck disable=SC2086
  for port in $(printf '%s\n' $misses | awk 'NF && !seen[$0]++'); do
    case "$undefined" in
      *" $port "*) echo "$port undefined" ;;
      *) echo "$port missing" ;;
    esac
  done
)

# PORT's output hash per piece (`<piece> <hash>`): recorded from its files
# when it is current here, else the registry's.
cmd_outputs() (
  port="$1"
  for piece in $(port_pieces "$port"); do
    if ! hash="$(port_hash "$piece")"; then
      echo "error: ${piece}: its key is undefined" >&2
      return 1
    fi
    if ! h="$(myos_piece_output_hash "$piece" "$hash")"; then
      echo "error: ${piece}: outputs neither built here nor in the registry" >&2
      return 1
    fi
    echo "$piece $h"
  done
)

# CMD (pull, push, exists) for each piece of PORT: exists fails when a
# piece is missing.
run_pieces() (
  status=0
  for piece in $(port_pieces "$2"); do
    "cmd_$1" "$piece" || status=1
  done
  return $status
)

run_many() (
  # shellcheck disable=SC2086
  for port in $ALL_PORTS; do
    run_pieces "$1" "$port"
  done
)

[ $# -ge 1 ] || usage
CMD="$1"
PORT="${2:-}"
case "$CMD" in
  current)
    [ -n "$PORT" ] || usage
    port_is_current "$PORT"
    ;;
  exists)
    [ -n "$PORT" ] || usage
    run_pieces exists "$PORT"
    ;;
  login)
    cmd_login
    ;;
  plan)
    shift
    cmd_plan "$@"
    ;;
  outputs)
    [ -n "$PORT" ] || usage
    cmd_outputs "$PORT"
    ;;
  pull|push)
    [ -n "$PORT" ] || usage
    if [ "$PORT" = all ]; then
      run_many "$CMD"
    else
      port_hash "$PORT" >/dev/null
      run_pieces "$CMD" "$PORT"
    fi
    ;;
  *) usage ;;
esac