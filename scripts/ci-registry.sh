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
# PORT is one of: newlib sbase oksh ubase coreutils ripgrep tcc std-hello c-hello
# or "all" (newlib first).
#
# Env:
#   GITHUB_TOKEN              required to login; pull still tries anonymous on miss
#   GITHUB_ACTOR              oras login user (fallback: GITHUB_REPOSITORY_OWNER)
#   GITHUB_REPOSITORY         owner/repo (default davide-matasci/myos)
#   MYOS_CI_REGISTRY_PUSH     false/0 skips push (fork PRs)
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# shellcheck source=scripts/myos-c-userspace-lib.sh
source "$ROOT/scripts/myos-c-userspace-lib.sh"
# shellcheck source=toolchain/std/lib.sh
source "$ROOT/toolchain/std/lib.sh"

ORAS_VERSION="1.3.3"
ORAS_LINUX_AMD64_SHA256="9ce999f8d2de03fc03968b29d743077a58783e545e5eaa53917ca177352d0e59"
ORAS_LINUX_ARM64_SHA256="ac7156f93a21e903f7ad606c792f3560f17e0cd0e36365634701b1e7cc4e4eca"
ORAS_ARTIFACT_TYPE="application/vnd.myos.ci.port.v1"
ORAS_LAYER_TYPE="application/vnd.myos.ci.port.layer.v1.tar+zst"

# newlib first: everyone else depends on it.
ALL_PORTS=(newlib std-hello c-hello sbase oksh ubase coreutils ripgrep tcc)

usage() {
  echo "usage: $0 pull|push PORT" >&2
  echo "  PORT: ${ALL_PORTS[*]} all" >&2
  exit 2
}

port_hash() {
  case "$1" in
    newlib) myos_newlib_version_hash ;;
    sbase) myos_sbase_version_hash ;;
    oksh) myos_oksh_version_hash ;;
    ubase) myos_ubase_version_hash ;;
    coreutils) myos_coreutils_version_hash ;;
    ripgrep) myos_ripgrep_version_hash ;;
    tcc) myos_tcc_version_hash ;;
    std-hello) myos_std_hello_version_hash ;;
    c-hello) myos_c_hello_version_hash ;;
    *) echo "error: unknown port $1" >&2; return 2 ;;
  esac
}

port_is_current() {
  case "$1" in
    newlib) myos_newlib_is_current ;;
    sbase) myos_sbase_is_current ;;
    oksh) myos_oksh_is_current ;;
    ubase) myos_ubase_is_current ;;
    coreutils) myos_coreutils_is_current ;;
    ripgrep) myos_ripgrep_is_current ;;
    tcc) myos_tcc_is_current ;;
    std-hello) myos_std_hello_is_current ;;
    c-hello) myos_c_hello_is_current ;;
    *) return 2 ;;
  esac
}

# Print repo-relative paths to pack. Directories are included recursively.
# Never lists *-src / *-myos-build / object trees.
port_members() {
  local port="$1"
  local arch triple
  case "$port" in
    newlib)
      echo target/.myos-newlib-version
      echo target/newlib-bin
      echo target/newlib-x86_64
      echo target/newlib-aarch64
      echo target/newlib-riscv64
      ;;
    sbase)
      echo target/.myos-sbase-version
      for arch in x86_64 aarch64 riscv64; do
        echo "target/sbase-manifest-${arch}.txt"
      done
      # ELFs only (maxdepth 1 files); skip sbase-src / sbase-myos-build dirs.
      find "$ROOT/target" -maxdepth 1 -type f -name 'sbase-*-unknown-none' -printf 'target/%f\n' 2>/dev/null || true
      ;;
    oksh)
      echo target/.myos-oksh-version
      for arch in x86_64 aarch64 riscv64; do
        echo "target/oksh-${arch}-unknown-none"
      done
      ;;
    ubase)
      echo target/.myos-ubase-version
      for arch in x86_64 aarch64 riscv64; do
        echo "target/ubase-manifest-${arch}.txt"
      done
      find "$ROOT/target" -maxdepth 1 -type f -name 'ubase-*-unknown-none' -printf 'target/%f\n' 2>/dev/null || true
      ;;
    coreutils)
      echo target/.myos-coreutils-version
      for arch in x86_64 aarch64 riscv64; do
        echo "target/coreutils-manifest-${arch}.txt"
        echo "target/coreutils-${arch}-unknown-myos"
      done
      ;;
    ripgrep)
      echo target/.myos-ripgrep-version
      for triple in x86_64-unknown-myos aarch64-unknown-myos riscv64-unknown-myos; do
        echo "target/rg-${triple}"
        echo "target/coreutils-rg-${triple}"
      done
      echo target/pcre2-x86_64
      echo target/pcre2-aarch64
      echo target/pcre2-riscv64
      ;;
    tcc)
      echo target/.myos-tcc-version
      for arch in x86_64 aarch64 riscv64; do
        triple="${arch}-unknown-myos"
        echo "target/tcc-${triple}"
        echo "target/coreutils-tcc-${triple}"
        echo "target/libtcc1-${triple}.a"
        echo "target/tcc-libtcc1-${triple}.a"
      done
      ;;
    std-hello)
      echo target/.myos-std-hello-version
      for triple in x86_64-unknown-myos aarch64-unknown-myos riscv64-unknown-myos; do
        for name in hello cat echo bigalloc; do
          echo "target/std-${name}-${triple}"
        done
      done
      ;;
    c-hello)
      echo target/.myos-c-hello-version
      for arch in x86_64 aarch64 riscv64; do
        echo "target/c-hello-${arch}-unknown-none"
      done
      ;;
    *) return 2 ;;
  esac
}

repo_lower() {
  local repo="${GITHUB_REPOSITORY:-davide-matasci/myos}"
  printf '%s' "${repo,,}"
}

registry_ref() {
  local port="$1" hash="$2"
  # GHCR repository names must be lowercase; stamp hashes may contain hex.
  printf 'ghcr.io/%s/ci-%s:%s' "$(repo_lower)" "${port,,}" "$hash"
}

package_name() {
  local port="$1"
  local repo
  repo="$(repo_lower)"
  printf '%s/ci-%s' "${repo#*/}" "${port,,}"
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

oras_login() {
  local user token err
  token="${GITHUB_TOKEN:-${GH_TOKEN:-}}"
  user="${GITHUB_ACTOR:-${GITHUB_REPOSITORY_OWNER:-${GITHUB_REPOSITORY%%/*}}}"
  user="${user,,}"
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

cmd_pull() {
  local port="$1"
  local hash ref tmp tarball
  hash="$(port_hash "$port")"
  ref="$(registry_ref "$port" "$hash")"
  ensure_oras
  # Anonymous fetch is fine when login fails (public packages / empty GHCR).
  oras_login || true
  if ! oras manifest fetch "$ref" >/dev/null 2>&1; then
    echo "registry miss ${port}; building"
    return 0
  fi
  tmp="$(mktemp -d "${TMPDIR:-/tmp}/myos-ci-reg.XXXXXX")"
  if ! oras pull "$ref" -o "$tmp" >/dev/null 2>&1; then
    rm -rf "$tmp"
    echo "registry miss ${port}; building"
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
    echo "registry miss ${port}; building"
    return 0
  fi
  mkdir -p "$ROOT/target"
  if ! tar -C "$ROOT" --zstd --no-same-owner -xf "$tarball" 2>/dev/null; then
    rm -rf "$tmp"
    echo "registry miss ${port}; building"
    return 0
  fi
  rewrite_manifest_paths "$port"
  rm -rf "$tmp"
  if port_is_current "$port"; then
    echo "registry hit ${port} ${hash}"
  else
    echo "registry miss ${port}; building"
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
    echo "registry skip push ${port}: already present"
    return 0
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
  rm -rf "$tmp"
  echo "registry push ${port} ${hash}"
}

run_many() {
  local cmd="$1"
  local port
  for port in "${ALL_PORTS[@]}"; do
    "cmd_${cmd}" "$port"
  done
}

[[ $# -ge 1 ]] || usage
CMD="$1"
PORT="${2:-}"
case "$CMD" in
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