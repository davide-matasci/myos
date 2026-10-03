#!/usr/bin/env bash
# Pull/push myos userspace *outputs* as GHCR OCI artifacts (oras).
#
# Tags are the stamp hashes from myos-c-userspace-lib.sh / toolchain/std/lib.sh.
# Cache ELFs, newlib prefixes, stamps — never *-src or *-myos-build trees.
# skip-if-fresh (myos_*_is_current) stays the local truth after a pull.
#
# Usage:
#   ./scripts/ci-registry.sh pull PORT
#   ./scripts/ci-registry.sh push PORT
#   ./scripts/ci-registry.sh current PORT   exit 0 when PORT's outputs are current
# PORT is a port with a descriptor and a build script (`scripts/ports.sh
# --list`: its stamp and PORT_OUTPUTS are cached, keyed by its
# myos_<name>_version_hash), or one of the pieces cached the same way:
# sysroot newlib linux-compat kernels. "all" does sysroot + newlib first.
#
# Env:
#   GITHUB_TOKEN              required for private GHCR; pull is anonymous only when unset
#   GITHUB_ACTOR              oras login user (fallback: GITHUB_REPOSITORY_OWNER)
#   GITHUB_REPOSITORY         owner/repo (default davide-matasci/myos)
#   MYOS_CI_REGISTRY_PUSH     false/0 skips push (fork PRs)
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# shellcheck source=scripts/myos-c-userspace-lib.sh
source "$ROOT/scripts/myos-c-userspace-lib.sh"
# shellcheck source=toolchain/std/lib.sh
source "$ROOT/toolchain/std/lib.sh"
# shellcheck source=scripts/ports.sh
source "$ROOT/scripts/ports.sh"

ORAS_VERSION="1.3.3"
ORAS_LINUX_AMD64_SHA256="9ce999f8d2de03fc03968b29d743077a58783e545e5eaa53917ca177352d0e59"
ORAS_LINUX_ARM64_SHA256="ac7156f93a21e903f7ad606c792f3560f17e0cd0e36365634701b1e7cc4e4eca"
ORAS_ARTIFACT_TYPE="application/vnd.myos.ci.port.v1"
ORAS_LAYER_TYPE="application/vnd.myos.ci.port.layer.v1.tar+zst"

# Every port with a build script in build order (the toolchains first:
# sysroot and newlib, which the others pull; image and package ports), then
# the Linux layer's musl pieces and the kernels.
ALL_PORTS=()
while read -r name; do
  ALL_PORTS+=("$name")
done < <(myos_port_build_order all)
ALL_PORTS+=(linux-compat kernels)

usage() {
  echo "usage: $0 pull|push|current|exists PORT | login" >&2
  echo "  PORT: ${ALL_PORTS[*]} all" >&2
  exit 2
}

# Dispatch to the hash / freshness functions of myos-c-userspace-lib.sh and
# toolchain/std/lib.sh: a port NAME has myos_<name>_version_hash and
# myos_<name>_is_current (dashes as underscores; the sysroot's are in
# toolchain/std/lib.sh).
port_fn() {
  local fn="myos_${1//-/_}_$2"
  if ! declare -F "$fn" >/dev/null; then
    echo "error: no function $fn for port $1 (scripts/myos-c-userspace-lib.sh)" >&2
    return 2
  fi
  echo "$fn"
}

port_hash() {
  local fn
  case "$1" in
    kernels) "$ROOT/scripts/ci-build-kernels.sh" --print-hash | tr -d '
' ;;
    *) fn="$(port_fn "$1" version_hash)" || return 2; "$fn" ;;
  esac
}

port_is_current() {
  local fn
  case "$1" in
    kernels) "$ROOT/scripts/ci-build-kernels.sh" --is-current ;;
    *) fn="$(port_fn "$1" is_current)" || return 2; "$fn" ;;
  esac
}

# Print repo-relative paths to pack. Directories are included recursively.
# Never lists *-src / *-myos-build / object trees: a port's descriptor names
# its outputs (PORT_OUTPUTS, scripts/ports.sh --outputs; the sysroot's is
# the slim pack: stamp, manifest, precompiled rlibs and target specs, never
# the patched library/ source tree under rustlib/src).
port_members() {
  local port="$1"
  local arch triple
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
    *) myos_port_outputs "$port" ;;
  esac
}

# macOS bash 3.2 has no ${var,,}; use tr.
lower() {
  printf '%s' "$1" | tr '[:upper:]' '[:lower:]'
}

repo_lower() {
  local repo="${GITHUB_REPOSITORY:-davide-matasci/myos}"
  lower "$repo"
}

registry_ref() {
  local port="$1" hash="$2"
  # GHCR repository names must be lowercase; stamp hashes may contain hex.
  printf 'ghcr.io/%s/ci-%s:%s' "$(repo_lower)" "$(lower "$port")" "$hash"
}

package_name() {
  local port="$1"
  local repo
  repo="$(repo_lower)"
  printf '%s/ci-%s' "${repo#*/}" "$(lower "$port")"
}

ensure_oras() {
  local bindir="${MYOS_ORAS_BINDIR:-$ROOT/target/.oras-bin}"
  export PATH="$bindir:$PATH"
  if [[ -x "$bindir/oras" ]]; then
    return 0
  fi
  local uname_s uname_m os arch url sha tgz
  uname_s="$(uname -s)"
  uname_m="$(uname -m)"
  case "$uname_s" in
    Linux) os=linux ;;
    *) echo "error: install oras (https://github.com/oras-project/oras/releases)" >&2; return 1 ;;
  esac
  case "$uname_m" in
    x86_64|amd64) arch=amd64; sha="$ORAS_LINUX_AMD64_SHA256" ;;
    aarch64|arm64) arch=arm64; sha="$ORAS_LINUX_ARM64_SHA256" ;;
    *) echo "error: unsupported arch $uname_m for pinned oras" >&2; return 1 ;;
  esac
  url="https://github.com/oras-project/oras/releases/download/v${ORAS_VERSION}/oras_${ORAS_VERSION}_${os}_${arch}.tar.gz"
  mkdir -p "$bindir"
  tgz="$(mktemp "${TMPDIR:-/tmp}/oras.XXXXXX.tar.gz")"
  curl -fsSL "$url" -o "$tgz"
  echo "${sha}  ${tgz}" | sha256sum -c - >/dev/null
  tar -xzf "$tgz" -C "$bindir" oras
  rm -f "$tgz"
  chmod +x "$bindir/oras"
}

# One oras login per job: `login` writes the marker, the pulls and pushes
# that follow (in parallel, from ci-build-pull-and-kernels.sh) skip theirs.
login_marker() {
  local base="${RUNNER_TEMP:-${TMPDIR:-/tmp}}"
  printf '%s' "${base%/}/myos-ci-registry-login-ok"
}

oras_login() {
  local user token err
  if [[ -f "$(login_marker)" ]]; then
    return 0
  fi
  token="${GITHUB_TOKEN:-${GH_TOKEN:-}}"
  user="${GITHUB_ACTOR:-${GITHUB_REPOSITORY_OWNER:-${GITHUB_REPOSITORY%%/*}}}"
  user="$(lower "$user")"
  if [[ -z "$token" || -z "$user" ]]; then
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
}

can_push() {
  case "${MYOS_CI_REGISTRY_PUSH:-}" in
    0|false|FALSE|no|NO) return 1 ;;
  esac
  [[ -n "${GITHUB_TOKEN:-${GH_TOKEN:-}}" ]]
}

# Manifests record absolute ELF paths; rewrite to this checkout so is_current holds.
rewrite_manifest_paths() {
  local port="$1"
  local f tmp line name path rel
  case "$port" in
    sbase|ubase) ;;
    *) return 0 ;;
  esac
  shopt -s nullglob
  for f in "$ROOT/target/${port}-manifest-"*.txt; do
    tmp="$(mktemp)"
    while IFS= read -r line || [[ -n "$line" ]]; do
      [[ -n "$line" ]] || continue
      if [[ "$line" != *:* ]]; then
        printf '%s\n' "$line"
        continue
      fi
      name="${line%%:*}"
      path="${line#*:}"
      if [[ "$path" == *"/target/"* ]]; then
        rel="target/${path#*/target/}"
        path="$ROOT/$rel"
      elif [[ "$path" == target/* ]]; then
        path="$ROOT/$path"
      elif [[ "$path" == /* && ! -f "$path" ]]; then
        path="$ROOT/target/$(basename "$path")"
      fi
      printf '%s:%s\n' "$name" "$path"
    done <"$f" >"$tmp"
    mv "$tmp" "$f"
  done
}

existing_members() {
  local port="$1" rel
  while IFS= read -r rel; do
    [[ -n "$rel" ]] || continue
    if [[ -e "$ROOT/$rel" ]]; then
      printf '%s\n' "$rel"
    fi
  done < <(port_members "$port" | awk 'NF && !seen[$0]++')
}

try_public_package() {
  local port="$1"
  local token owner pkg enc url
  token="${GITHUB_TOKEN:-${GH_TOKEN:-}}"
  [[ -n "$token" ]] || return 0
  owner="${GITHUB_REPOSITORY_OWNER:-${GITHUB_REPOSITORY%%/*}}"
  pkg="$(package_name "$port")"
  enc="${pkg//\//%2F}"
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
}

force_replace_marker() {
  local port="$1"
  # Prefer runner temp /tmp — never under target/ (builds may wipe it).
  local base="${RUNNER_TEMP:-${TMPDIR:-/tmp}}"
  printf '%s' "${base%/}/myos-ci-registry-replace-${port}"
}

mark_force_replace() {
  local port="$1" hash="$2"
  printf '%s\n' "$hash" >"$(force_replace_marker "$port")"
}

clear_force_replace() {
  local port="$1"
  rm -f "$(force_replace_marker "$port")"
}

needs_force_replace() {
  local port="$1" hash="$2" marker
  marker="$(force_replace_marker "$port")"
  # Presence alone: pull may have stored a different hash if stamps used to be
  # non-deterministic across the same job.
  [[ -f "$marker" ]]
}

# 0 when the registry has PORT at its current input hash (manifest only, no
# download): what the CI plan asks before running a toolchain job.
cmd_exists() {
  local port="$1"
  local hash ref
  hash="$(port_hash "$port")"
  ref="$(registry_ref "$port" "$hash")"
  ensure_oras
  if [[ -n "${GITHUB_TOKEN:-${GH_TOKEN:-}}" ]]; then
    oras_login || true
  fi
  if oras manifest fetch "$ref" >/dev/null 2>&1; then
    echo "registry has ${port} ${hash}"
    return 0
  fi
  echo "registry lacks ${port} ${hash}"
  return 1
}

cmd_login() {
  ensure_oras
  oras_login
}

cmd_pull() {
  local port="$1"
  local hash ref tmp tarball
  hash="$(port_hash "$port")"
  ref="$(registry_ref "$port" "$hash")"
  clear_force_replace "$port"
  ensure_oras
  # Token present: require login so private GHCR packages are visible.
  # Anonymous fetch only when no token (public packages / empty GHCR).
  if [[ -n "${GITHUB_TOKEN:-${GH_TOKEN:-}}" ]]; then
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
  local f
  for f in "$tmp"/*.tar.zst "$tmp"/*.tar.zstd; do
    [[ -f "$f" ]] || continue
    tarball="$f"
    break
  done
  if [[ -z "$tarball" ]]; then
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
    echo "registry hit ${port} ${hash}"
  else
    mark_force_replace "$port" "$hash"
    echo "registry miss ${port}: not current after extract ${hash}"
  fi
}

cmd_push() {
  local port="$1"
  local hash ref tmp list status
  if ! can_push; then
    echo "registry skip push (disabled)"
    return 0
  fi
  if ! port_is_current "$port"; then
    echo "registry skip push ${port}: not current"
    return 0
  fi
  hash="$(port_hash "$port")"
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
  tmp="$(mktemp -d "${TMPDIR:-/tmp}/myos-ci-reg.XXXXXX")"
  list="$tmp/members.txt"
  existing_members "$port" >"$list"
  if [[ ! -s "$list" ]]; then
    rm -rf "$tmp"
    echo "registry skip push ${port}: nothing to pack"
    return 0
  fi
  tar -C "$ROOT" --zstd -cf "$tmp/${port}.tar.zst" -T "$list"
  # oras push rejects absolute file paths; push from $tmp with a relative name.
  set +e
  (
    cd "$tmp" && oras push "$ref" \
      --artifact-type "$ORAS_ARTIFACT_TYPE" \
      "${port}.tar.zst:${ORAS_LAYER_TYPE}" >/dev/null
  ) 2>"$tmp/oras.err"
  status=$?
  set -e
  if (( status != 0 )); then
    echo "registry push failed ${port}"
    echo "registry push failed ${port}" >&2
    if [[ -s "$tmp/oras.err" ]]; then
      cat "$tmp/oras.err"
      cat "$tmp/oras.err" >&2
    fi
    rm -rf "$tmp"
    return 1
  fi
  try_public_package "$port"
  clear_force_replace "$port"
  rm -rf "$tmp"
  echo "registry push ${port} ${hash}"
}

run_many() {
  local cmd="$1"
  local port
  for port in "${ALL_PORTS[@]+"${ALL_PORTS[@]}"}"; do
    "cmd_${cmd}" "$port"
  done
}

[[ $# -ge 1 ]] || usage
CMD="$1"
PORT="${2:-}"
case "$CMD" in
  current)
    [[ -n "$PORT" ]] || usage
    port_is_current "$PORT"
    ;;
  exists)
    [[ -n "$PORT" ]] || usage
    cmd_exists "$PORT"
    ;;
  login)
    cmd_login
    ;;
  pull|push)
    [[ -n "$PORT" ]] || usage
    if [[ "$PORT" == all ]]; then
      run_many "$CMD"
    else
      port_hash "$PORT" >/dev/null
      "cmd_${CMD}" "$PORT"
    fi
    ;;
  *) usage ;;
esac