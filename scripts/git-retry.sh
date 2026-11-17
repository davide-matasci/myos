#!/bin/sh
# Retry git clone/fetch with exponential backoff (CI HTTP 429 / RPC hangups).
#
# Drop-in for git:
#   scripts/git-retry.sh clone --depth 1 --branch TAG URL DEST
#   scripts/git-retry.sh -C DEST fetch --depth 1 origin REV
#
# GitHub HTTPS clones: if GH_TOKEN or GITHUB_TOKEN is set, pass it as
# http.https://github.com/.extraheader. Optional: no workflow secret required.
#
# Env:
#   MYOS_GIT_RETRIES      attempts (default 5)
#   MYOS_GIT_RETRY_BASE   initial backoff seconds (default 4)

myos_git() (
  max="${MYOS_GIT_RETRIES:-5}"
  delay="${MYOS_GIT_RETRY_BASE:-4}"
  attempt=1
  rc=0
  is_clone=0
  dest=""
  tok="${GH_TOKEN:-${GITHUB_TOKEN:-}}"
  b64=""
  need_github=0
  auth=""

  export GIT_TERMINAL_PROMPT="${GIT_TERMINAL_PROMPT:-0}"

  for arg in "$@"; do
    case "$arg" in
      clone) is_clone=1 ;;
      *github.com*) need_github=1 ;;
    esac
  done
  if [ "$is_clone" -ne 0 ]; then
    # The last argument.
    for arg in "$@"; do
      dest="$arg"
    done
  fi

  if [ "$need_github" -ne 0 ] && [ -n "$tok" ]; then
    b64="$(printf 'x-access-token:%s' "$tok" | base64 | tr -d '\n\r')"
    auth="http.https://github.com/.extraheader=AUTHORIZATION: basic ${b64}"
  fi

  while [ "$attempt" -le "$max" ]; do
    if [ "$is_clone" -ne 0 ]; then
      case "$dest" in
        "" | / | *://*) ;;
        *) rm -rf -- "$dest" ;;
      esac
    fi
    git ${auth:+-c "$auth"} "$@" && return 0
    rc=$?
    if [ "$attempt" -ge "$max" ]; then
      echo "error: git $* failed after ${attempt} attempts (exit ${rc})" >&2
      return "$rc"
    fi
    echo "warning: git failed (attempt ${attempt}/${max}, exit ${rc}); retrying in ${delay}s..." >&2
    sleep "$delay"
    delay=$((delay * 2))
    attempt=$((attempt + 1))
  done
  return "$rc"
)

# Executed, not sourced (a script that sources this file has another name).
if [ "${0##*/}" = git-retry.sh ]; then
  myos_git "$@"
fi
