#!/usr/bin/env bash
# Retry git clone/fetch with exponential backoff (CI HTTP 429 / RPC hangups).
#
# Drop-in for git:
#   scripts/git-retry.sh clone --depth 1 --branch TAG URL DEST
#   scripts/git-retry.sh -C DEST fetch --depth 1 origin REV
#
# GitHub HTTPS clones: if GH_TOKEN or GITHUB_TOKEN is set, pass it as
# http.https://github.com/.extraheader. Optional — no workflow secret required.
#
# Env:
#   MYOS_GIT_RETRIES      attempts (default 5)
#   MYOS_GIT_RETRY_BASE   initial backoff seconds (default 4)

myos_git() {
  local max="${MYOS_GIT_RETRIES:-5}"
  local delay="${MYOS_GIT_RETRY_BASE:-4}"
  local attempt=1
  local rc=0
  local is_clone=0
  local dest=""
  local arg
  local tok="${GH_TOKEN:-${GITHUB_TOKEN:-}}"
  local b64=""
  local need_github=0
  local -a auth=()

  export GIT_TERMINAL_PROMPT="${GIT_TERMINAL_PROMPT:-0}"

  for arg in "$@"; do
    case "$arg" in
      clone) is_clone=1 ;;
      *github.com*) need_github=1 ;;
    esac
  done
  if (( is_clone )); then
    dest="${@: -1}"
  fi

  if (( need_github )) && [[ -n "$tok" ]]; then
    b64="$(printf 'x-access-token:%s' "$tok" | base64 | tr -d '\n\r')"
    auth=(-c "http.https://github.com/.extraheader=AUTHORIZATION: basic ${b64}")
  fi

  while (( attempt <= max )); do
    if (( is_clone )) && [[ -n "$dest" && "$dest" != "/" && "$dest" != *://* ]]; then
      rm -rf -- "$dest"
    fi
    git "${auth[@]}" "$@" && return 0
    rc=$?
    if (( attempt >= max )); then
      echo "error: git $* failed after ${attempt} attempts (exit ${rc})" >&2
      return "$rc"
    fi
    echo "warning: git failed (attempt ${attempt}/${max}, exit ${rc}); retrying in ${delay}s..." >&2
    sleep "$delay"
    delay=$((delay * 2))
    attempt=$((attempt + 1))
  done
  return "$rc"
}

if [[ "${BASH_SOURCE[0]}" == "$0" ]]; then
  myos_git "$@"
fi
