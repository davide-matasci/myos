#!/usr/bin/env bash
# Shared version stamps for newlib + C userspace smoke ELFs (CI cache invalidation).
set -euo pipefail

MYOS_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
export MYOS_ROOT

MYOS_NEWLIB_TAG="${NEWLIB_TAG:-newlib-4.4.0}"
MYOS_NEWLIB_VERSION="$MYOS_ROOT/target/.myos-newlib-version"
MYOS_C_HELLO_VERSION="$MYOS_ROOT/target/.myos-c-hello-version"
MYOS_SBASE_VERSION="$MYOS_ROOT/target/.myos-sbase-version"
MYOS_OKSH_VERSION="$MYOS_ROOT/target/.myos-oksh-version"
MYOS_UBASE_VERSION="$MYOS_ROOT/target/.myos-ubase-version"
MYOS_COREUTILS_VERSION="$MYOS_ROOT/target/.myos-coreutils-version"
MYOS_RIPGREP_VERSION="$MYOS_ROOT/target/.myos-ripgrep-version"
MYOS_TCC_VERSION="$MYOS_ROOT/target/.myos-tcc-version"
MYOS_VIM_VERSION="$MYOS_ROOT/target/.myos-vim-version"
MYOS_NCURSES_VERSION="$MYOS_ROOT/target/.myos-ncurses-version"

MYOS_SBASE_MANIFEST="$MYOS_ROOT/target/sbase-manifest-x86_64.txt"
MYOS_COREUTILS_MANIFEST="$MYOS_ROOT/target/coreutils-manifest-x86_64.txt"
MYOS_SBASE_MIN_BUILT=90

# Rust std is statically linked into uutils/ripgrep; include sysroot stamp.
# shellcheck source=toolchain/std/lib.sh
source "$MYOS_ROOT/toolchain/std/lib.sh"

myos_newlib_version_hash() {
  local h
  h="$(
    {
      echo "newlib_tag=$MYOS_NEWLIB_TAG"
      find "$MYOS_ROOT/toolchain/newlib/libgloss/myos" -type f -print0 2>/dev/null \
        | sort -z | xargs -0 sha256sum
      sha256sum "$MYOS_ROOT/toolchain/newlib/patch.sh"
      sha256sum "$MYOS_ROOT/toolchain/newlib/build.sh"
      sha256sum "$MYOS_ROOT/toolchain/newlib/build-libgloss.sh"
      sha256sum "$MYOS_ROOT/toolchain/newlib/tool-wrappers.sh"
    } | sha256sum | awk '{print $1}'
  )"
  printf '%s' "$h"
}

myos_newlib_prefix_ok() {
  local arch="$1"
  local triple="${arch}-unknown-myos"
  local prefix="$MYOS_ROOT/target/newlib-${arch}/${triple}/lib"
  [[ -f "$prefix/libc.a" && -f "$prefix/libgloss.a" && -f "$prefix/crt0.o" && -f "$prefix/crti.o" && -f "$prefix/crtn.o" ]]
}

myos_newlib_is_current() {
  [[ -f "$MYOS_NEWLIB_VERSION" ]] \
    && [[ "$(cat "$MYOS_NEWLIB_VERSION")" == "$(myos_newlib_version_hash)" ]] \
    && myos_newlib_prefix_ok x86_64 \
    && myos_newlib_prefix_ok aarch64 \
    && myos_newlib_prefix_ok riscv64
}

myos_c_hello_version_hash() {
  local h
  h="$(
    {
      myos_newlib_version_hash
      sha256sum "$MYOS_ROOT/c/hello.c"
      sha256sum "$MYOS_ROOT/scripts/build-c-hello.sh"
    } | sha256sum | awk '{print $1}'
  )"
  printf '%s' "$h"
}

myos_c_hello_is_current() {
  [[ -f "$MYOS_C_HELLO_VERSION" ]] \
    && [[ "$(cat "$MYOS_C_HELLO_VERSION")" == "$(myos_c_hello_version_hash)" ]] \
    && [[ -f "$MYOS_ROOT/target/c-hello-x86_64-unknown-none" ]] \
    && [[ -f "$MYOS_ROOT/target/c-hello-aarch64-unknown-none" ]] \
    && [[ -f "$MYOS_ROOT/target/c-hello-riscv64-unknown-none" ]]
}

myos_sbase_version_hash() {
  local h
  h="$(
    {
      myos_newlib_version_hash
      sha256sum "$MYOS_ROOT/ports/sbase/build.sh"
      sha256sum "$MYOS_ROOT/ports/sbase/prepare.sh"
      sha256sum "$MYOS_ROOT/ports/sbase/fetch.sh"
      sha256sum "$MYOS_ROOT/ports/sbase/bins.txt"
      find "$MYOS_ROOT/ports/sbase" -type f -print0 2>/dev/null \
        | sort -z | xargs -0 sha256sum
    } | sha256sum | awk '{print $1}'
  )"
  printf '%s' "$h"
}

myos_sbase_manifest_count() {
  local manifest="$1"
  [[ -f "$manifest" ]] || return 1
  wc -l <"$manifest" | tr -d ' '
}

# Names that land in a port manifest (skip blanks and # comments).
myos_bins_txt_count() {
  local file="$1"
  [[ -f "$file" ]] || return 1
  grep -E -cve '^[[:space:]]*(#|$)' "$file"
}

myos_sbase_is_current() {
  local arch manifest count
  [[ -f "$MYOS_SBASE_VERSION" ]] \
    && [[ "$(cat "$MYOS_SBASE_VERSION")" == "$(myos_sbase_version_hash)" ]] \
    || return 1
  for arch in x86_64 aarch64 riscv64; do
    manifest="$MYOS_ROOT/target/sbase-manifest-${arch}.txt"
    count="$(myos_sbase_manifest_count "$manifest")" || return 1
    if ((count < MYOS_SBASE_MIN_BUILT)); then
      return 1
    fi
    while IFS= read -r line; do
      [[ -n "$line" ]] || continue
      local path="${line#*:}"
      [[ -f "$path" ]] || return 1
    done <"$manifest"
  done
}

myos_oksh_version_hash() {
  local h
  h="$(
    {
      myos_newlib_version_hash
      sha256sum "$MYOS_ROOT/ports/oksh/build.sh"
      sha256sum "$MYOS_ROOT/ports/oksh/prepare.sh"
      sha256sum "$MYOS_ROOT/ports/oksh/fetch.sh"
      find "$MYOS_ROOT/ports/oksh" -type f -print0 2>/dev/null \
        | sort -z | xargs -0 sha256sum
    } | sha256sum | awk '{print $1}'
  )"
  printf '%s' "$h"
}

myos_oksh_is_current() {
  local arch
  [[ -f "$MYOS_OKSH_VERSION" ]] \
    && [[ "$(cat "$MYOS_OKSH_VERSION")" == "$(myos_oksh_version_hash)" ]] \
    || return 1
  for arch in x86_64 aarch64 riscv64; do
    [[ -f "$MYOS_ROOT/target/oksh-${arch}-unknown-none" ]] || return 1
  done
}

myos_ubase_version_hash() {
  local h
  h="$(
    {
      myos_newlib_version_hash
      sha256sum "$MYOS_ROOT/ports/ubase/build.sh"
      sha256sum "$MYOS_ROOT/ports/ubase/prepare.sh"
      sha256sum "$MYOS_ROOT/ports/ubase/fetch.sh"
      sha256sum "$MYOS_ROOT/ports/ubase/bins.txt"
      find "$MYOS_ROOT/ports/ubase" -type f -print0 2>/dev/null \
        | sort -z | xargs -0 sha256sum
    } | sha256sum | awk '{print $1}'
  )"
  printf '%s' "$h"
}

myos_ubase_is_current() {
  local arch manifest
  [[ -f "$MYOS_UBASE_VERSION" ]] \
    && [[ "$(cat "$MYOS_UBASE_VERSION")" == "$(myos_ubase_version_hash)" ]] \
    || return 1
  for arch in x86_64 aarch64 riscv64; do
    manifest="$MYOS_ROOT/target/ubase-manifest-${arch}.txt"
    [[ -f "$manifest" ]] || return 1
    while IFS= read -r line; do
      [[ -n "$line" ]] || continue
      local path="${line#*:}"
      [[ -f "$path" ]] || return 1
    done <"$manifest"
    [[ -f "$MYOS_ROOT/target/ubase-getty-${arch}-unknown-none" ]] || return 1
    [[ -f "$MYOS_ROOT/target/ubase-login-${arch}-unknown-none" ]] || return 1
  done
}

myos_coreutils_version_hash() {
  local h
  h="$(
    {
      # uutils links libstd from the myos sysroot — abi.rs etc. must bust this stamp
      # (d72287e a2=0 fix was skipped in CI: "uutils coreutils up to date").
      myos_sysroot_version_hash
      sha256sum "$MYOS_ROOT/ports/coreutils/build-uutils.sh"
      sha256sum "$MYOS_ROOT/ports/coreutils/build.sh"
      sha256sum "$MYOS_ROOT/ports/coreutils/prepare.sh"
      sha256sum "$MYOS_ROOT/ports/coreutils/versions.env"
      find "$MYOS_ROOT/ports/coreutils" -type f -print0 2>/dev/null \
        | sort -z | xargs -0 sha256sum
      find "$MYOS_ROOT/ports/crates/libc" -type f -print0 2>/dev/null \
        | sort -z | xargs -0 sha256sum
      sha256sum "$MYOS_ROOT/ports/coreutils/bins.txt"
      sha256sum "$MYOS_ROOT/ports/coreutils/cargo-config.toml"
    } | sha256sum | awk '{print $1}'
  )"
  printf '%s' "$h"
}

myos_coreutils_manifest_count() {
  local manifest="$1"
  [[ -f "$manifest" ]] || return 1
  wc -l <"$manifest" | tr -d ' '
}

myos_coreutils_is_current() {
  local arch manifest count expected triple
  [[ -f "$MYOS_COREUTILS_VERSION" ]] \
    && [[ "$(cat "$MYOS_COREUTILS_VERSION")" == "$(myos_coreutils_version_hash)" ]] \
    || return 1
  for arch in x86_64 aarch64 riscv64; do
    triple="${arch}-unknown-myos"
    manifest="$MYOS_ROOT/target/coreutils-manifest-${arch}.txt"
    count="$(myos_coreutils_manifest_count "$manifest")" || return 1
    expected="$(myos_bins_txt_count "$MYOS_ROOT/ports/coreutils/bins.txt")" || return 1
    if ((count < expected)); then
      return 1
    fi
    [[ -f "$MYOS_ROOT/target/coreutils-${triple}" ]] || return 1
  done
}


myos_ripgrep_version_hash() {
  local h
  h="$(
    {
      sha256sum "$MYOS_ROOT/ports/ripgrep/build.sh"
      sha256sum "$MYOS_ROOT/ports/ripgrep/prepare.sh"
      sha256sum "$MYOS_ROOT/ports/ripgrep/fetch.sh"
      sha256sum "$MYOS_ROOT/ports/ripgrep/fetch-pcre2.sh"
      sha256sum "$MYOS_ROOT/ports/ripgrep/build-pcre2.sh"
      sha256sum "$MYOS_ROOT/ports/ripgrep/versions.env"
      sha256sum "$MYOS_ROOT/ports/ripgrep/cargo-config.toml"
      find "$MYOS_ROOT/ports/ripgrep" -type f -print0 2>/dev/null \
        | sort -z | xargs -0 sha256sum
      myos_sysroot_version_hash
      myos_newlib_version_hash
    } | sha256sum | awk '{print $1}'
  )"
  printf '%s' "$h"
}

myos_ripgrep_is_current() {
  local arch triple
  [[ -f "$MYOS_RIPGREP_VERSION" ]] \
    && [[ "$(cat "$MYOS_RIPGREP_VERSION")" == "$(myos_ripgrep_version_hash)" ]] \
    || return 1
  for arch in x86_64 aarch64 riscv64; do
    triple="${arch}-unknown-myos"
    [[ -f "$MYOS_ROOT/target/rg-${triple}" ]] || return 1
  done
}


myos_tcc_version_hash() {
  local h
  h="$(
    {
      myos_newlib_version_hash
      sha256sum "$MYOS_ROOT/ports/tcc/build.sh"
      sha256sum "$MYOS_ROOT/ports/tcc/prepare.sh"
      sha256sum "$MYOS_ROOT/ports/tcc/fetch.sh"
      sha256sum "$MYOS_ROOT/ports/tcc/versions.env"
      find "$MYOS_ROOT/ports/tcc" -type f -print0 2>/dev/null \
        | sort -z | xargs -0 sha256sum
    } | sha256sum | awk '{print $1}'
  )"
  printf '%s' "$h"
}

myos_tcc_is_current() {
  local arch triple
  [[ -f "$MYOS_TCC_VERSION" ]] \
    && [[ "$(cat "$MYOS_TCC_VERSION")" == "$(myos_tcc_version_hash)" ]] \
    || return 1
  for arch in x86_64 aarch64 riscv64; do
    triple="${arch}-unknown-myos"
    [[ -f "$MYOS_ROOT/target/tcc-${triple}" ]] || return 1
    [[ -f "$MYOS_ROOT/target/libtcc1-${triple}.a" ]] || return 1
  done
}


myos_vim_version_hash() {
  local h
  h="$(
    {
      myos_newlib_version_hash
      myos_ncurses_version_hash
      sha256sum "$MYOS_ROOT/ports/vim/build.sh"
      sha256sum "$MYOS_ROOT/ports/vim/prepare.sh"
      sha256sum "$MYOS_ROOT/ports/vim/fetch.sh"
      sha256sum "$MYOS_ROOT/ports/vim/versions.env"
      find "$MYOS_ROOT/ports/vim" -type f -print0 2>/dev/null \
        | sort -z | xargs -0 sha256sum
    } | sha256sum | awk '{print $1}'
  )"
  printf '%s' "$h"
}

myos_vim_is_current() {
  local arch
  [[ -f "$MYOS_VIM_VERSION" ]] \
    && [[ "$(cat "$MYOS_VIM_VERSION")" == "$(myos_vim_version_hash)" ]] \
    || return 1
  for arch in x86_64 aarch64 riscv64; do
    [[ -f "$MYOS_ROOT/target/vim-${arch}-unknown-none" ]] || return 1
  done
}


myos_ncurses_version_hash() {
  local h
  h="$(
    {
      myos_newlib_version_hash
      sha256sum "$MYOS_ROOT/ports/ncurses/build.sh"
      sha256sum "$MYOS_ROOT/ports/ncurses/prepare.sh"
      sha256sum "$MYOS_ROOT/ports/ncurses/fetch.sh"
      sha256sum "$MYOS_ROOT/ports/ncurses/versions.env"
      find "$MYOS_ROOT/ports/ncurses" -type f -print0 2>/dev/null \
        | sort -z | xargs -0 sha256sum
    } | sha256sum | awk '{print $1}'
  )"
  printf '%s' "$h"
}

myos_ncurses_is_current() {
  local arch
  [[ -f "$MYOS_NCURSES_VERSION" ]] \
    && [[ "$(cat "$MYOS_NCURSES_VERSION")" == "$(myos_ncurses_version_hash)" ]] \
    || return 1
  for arch in x86_64 aarch64 riscv64; do
    [[ -f "$MYOS_ROOT/target/ncurses-${arch}/lib/libncurses.a" ]] || return 1
  done
}
