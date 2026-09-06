#!/usr/bin/env bash
# Fetch a pinned Vim release for the myos port.
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$HERE/../.." && pwd)"
# shellcheck source=ports/vim/versions.env
source "$HERE/versions.env"
VIM_SRC="$ROOT/target/vim-src"
VIM_REV="${VIM_REV:-e7e21018fc0b60c153c8e668f696d95e574cc5a4}"
VIM_TAG="${VIM_TAG:-v9.2.0}"

if [[ -d "$VIM_SRC/.git" && -f "$VIM_SRC/src/vim.h" ]]; then
  got="$(git -C "$VIM_SRC" rev-parse HEAD)"
  if [[ "$got" == "$VIM_REV" ]]; then
    echo "vim already present at $VIM_SRC ($VIM_REV)"
    exit 0
  fi
  echo "vim at $got, want $VIM_REV; refetching" >&2
  rm -rf "$VIM_SRC"
fi

if [[ -e "$VIM_SRC" ]]; then
  echo "removing incomplete vim tree at $VIM_SRC" >&2
  rm -rf "$VIM_SRC"
fi

echo "==> fetch vim ($VIM_TAG / $VIM_REV)"
"$ROOT/scripts/git-retry.sh" clone --depth 1 --branch "$VIM_TAG" https://github.com/vim/vim.git "$VIM_SRC"
got="$(git -C "$VIM_SRC" rev-parse HEAD)"
if [[ "$got" != "$VIM_REV" ]]; then
  echo "error: vim HEAD $got does not match pin $VIM_REV" >&2
  exit 1
fi
