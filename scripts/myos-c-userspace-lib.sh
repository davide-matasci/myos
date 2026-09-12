#!/usr/bin/env bash
# Shared version stamps for newlib + C userspace smoke ELFs (CI cache invalidation).
set -euo pipefail

MYOS_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
export MYOS_ROOT

MYOS_NEWLIB_TAG="${NEWLIB_TAG:-newlib-4.4.0}"
MYOS_NEWLIB_VERSION="$MYOS_ROOT/target/.myos-newlib-version"
MYOS_C_HELLO_VERSION="$MYOS_ROOT/target/.myos-c-hello-version"
MYOS_CURL_VERSION="$MYOS_ROOT/target/.myos-curl-version"
MYOS_SBASE_VERSION="$MYOS_ROOT/target/.myos-sbase-version"
MYOS_OKSH_VERSION="$MYOS_ROOT/target/.myos-oksh-version"
MYOS_UBASE_VERSION="$MYOS_ROOT/target/.myos-ubase-version"
MYOS_COREUTILS_VERSION="$MYOS_ROOT/target/.myos-coreutils-version"
MYOS_RIPGREP_VERSION="$MYOS_ROOT/target/.myos-ripgrep-version"
MYOS_TCC_VERSION="$MYOS_ROOT/target/.myos-tcc-version"
MYOS_VIM_VERSION="$MYOS_ROOT/target/.myos-vim-version"
MYOS_NCURSES_VERSION="$MYOS_ROOT/target/.myos-ncurses-version"
MYOS_ZLIB_VERSION="$MYOS_ROOT/target/.myos-zlib-version"
MYOS_GIT_VERSION="$MYOS_ROOT/target/.myos-git-version"
MYOS_LYNX_VERSION="$MYOS_ROOT/target/.myos-lynx-version"
MYOS_MAKE_VERSION="$MYOS_ROOT/target/.myos-make-version"

MYOS_SBASE_MANIFEST="$MYOS_ROOT/target/sbase-manifest-x86_64.txt"
MYOS_COREUTILS_MANIFEST="$MYOS_ROOT/target/coreutils-manifest-x86_64.txt"
MYOS_SBASE_MIN_BUILT=90

# Rust std is statically linked into uutils/ripgrep; include sysroot stamp.
# shellcheck source=toolchain/std/lib.sh
source "$MYOS_ROOT/toolchain/std/lib.sh"

# macOS bash 3.2 helpers live here; scripts source this file.
# Homebrew's llvm is keg-only, so ld.lld is often not on PATH. Probe the
# standard keg locations once, up front.
myos_ensure_llvm_bin() {
  if ! command -v ld.lld >/dev/null 2>&1; then
    local d
    for d in /opt/homebrew/opt/llvm/bin /usr/local/opt/llvm/bin; do
      if [ -x "$d/ld.lld" ]; then
        PATH="$d:$PATH"
        export PATH
        return 0
      fi
    done
    # Custom HOMEBREW_PREFIX or other brew location: ask brew itself.
    if command -v brew >/dev/null 2>&1; then
      d="$(brew --prefix llvm 2>/dev/null)/bin"
      if [ -x "$d/ld.lld" ]; then
        PATH="$d:$PATH"
        export PATH
        return 0
      fi
    fi
    echo 'ld.lld not found: brew install llvm, then export PATH="$(brew --prefix llvm)/bin:$PATH"' >&2
    return 1
  fi
}

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
      sha256sum "$MYOS_ROOT/c/socket_smoke.c"
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
    && [[ -f "$MYOS_ROOT/target/c-hello-riscv64-unknown-none" ]] \
    && [[ -f "$MYOS_ROOT/target/c-socket_smoke-x86_64-unknown-none" ]] \
    && [[ -f "$MYOS_ROOT/target/c-socket_smoke-aarch64-unknown-none" ]] \
    && [[ -f "$MYOS_ROOT/target/c-socket_smoke-riscv64-unknown-none" ]]
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


myos_curl_version_hash() {
  # curl's hash_curl() includes the CURL_VERSION value from versions.env.
  # Source it here so the registry stamp matches curl/build.sh's own gate.
  if [[ -f "$MYOS_ROOT/ports/curl/versions.env" ]]; then
    # shellcheck disable=SC1091
    source "$MYOS_ROOT/ports/curl/versions.env"
  fi
  local h
  h="$(
    {
      # Mirror ports/curl/build.sh hash_curl() exactly so the registry stamp
      # matches the script own short-circuit (no apostrophes in comments:
      # macOS bash 3.2 mis-parses quotes inside command substitutions).
      echo "$CURL_VERSION"
      sha256sum "$MYOS_ROOT/ports/curl/build.sh" \
        "$MYOS_ROOT/ports/curl/fetch.sh" \
        "$MYOS_ROOT/ports/curl/versions.env" \
        "$MYOS_ROOT/ports/curl/config-myos.h" || true
      # Statically links mbedtls: rebuild when CA/FS config changes. Hash mbedtls
      # SOURCES only (never target/.myos-mbedtls-version, a build output) so the
      # registry tag is deterministic at pull time on a fresh workspace.
      sha256sum "$MYOS_ROOT/ports/mbedtls/build.sh" \
        "$MYOS_ROOT/ports/mbedtls/fetch.sh" \
        "$MYOS_ROOT/ports/mbedtls/versions.env" \
        "$MYOS_ROOT/ports/mbedtls/myos_mbedtls_config.h" || true
      find "$MYOS_ROOT/ports/mbedtls/include" -type f -print0 2>/dev/null \
        | sort -z | xargs -0 sha256sum 2>/dev/null || true
      myos_newlib_version_hash
    } | sha256sum | awk '{print $1}'
  )"
  printf '%s' "$h"
}

myos_curl_is_current() {
  local arch
  [[ -f "$MYOS_CURL_VERSION" ]] \
    && [[ "$(cat "$MYOS_CURL_VERSION")" == "$(myos_curl_version_hash)" ]] \
    || return 1
  for arch in x86_64 aarch64 riscv64; do
    [[ -f "$MYOS_ROOT/target/curl-${arch}-unknown-none" ]] || return 1
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


myos_zlib_version_hash() {
  local h
  h="$(
    {
      myos_newlib_version_hash
      sha256sum "$MYOS_ROOT/ports/zlib/build.sh"
      sha256sum "$MYOS_ROOT/ports/zlib/fetch.sh"
      sha256sum "$MYOS_ROOT/ports/zlib/versions.env"
      find "$MYOS_ROOT/ports/zlib" -type f -print0 2>/dev/null \
        | sort -z | xargs -0 sha256sum
    } | sha256sum | awk '{print $1}'
  )"
  printf '%s' "$h"
}

myos_zlib_is_current() {
  local arch
  [[ -f "$MYOS_ZLIB_VERSION" ]] \
    && [[ "$(cat "$MYOS_ZLIB_VERSION")" == "$(myos_zlib_version_hash)" ]] \
    || return 1
  for arch in x86_64 aarch64 riscv64; do
    [[ -f "$MYOS_ROOT/target/zlib-${arch}/lib/libz.a" ]] || return 1
  done
}

myos_git_version_hash() {
  local h
  h="$(
    {
      myos_newlib_version_hash
      myos_zlib_version_hash
      sha256sum "$MYOS_ROOT/ports/git/build.sh"
      sha256sum "$MYOS_ROOT/ports/git/prepare.sh"
      sha256sum "$MYOS_ROOT/ports/git/fetch.sh"
      sha256sum "$MYOS_ROOT/ports/git/versions.env"
      find "$MYOS_ROOT/ports/git" -type f -print0 2>/dev/null \
        | sort -z | xargs -0 sha256sum
    } | sha256sum | awk '{print $1}'
  )"
  printf '%s' "$h"
}

myos_git_elfs_present() {
  local arch
  for arch in x86_64 aarch64 riscv64; do
    [[ -f "$MYOS_ROOT/target/git-${arch}-unknown-none" ]] || return 1
  done
}

myos_git_is_current() {
  [[ -f "$MYOS_GIT_VERSION" ]] \
    && [[ "$(cat "$MYOS_GIT_VERSION")" == "$(myos_git_version_hash)" ]] \
    || return 1
  myos_git_elfs_present
}

myos_lynx_version_hash() {
  local h
  h="$(
    {
      myos_newlib_version_hash
      myos_ncurses_version_hash
      # versions.env content hashed below
      # shellcheck source=ports/lynx/versions.env
      # LYNX_VERSION may be unset when called from registry; hash the env file.
      sha256sum "$MYOS_ROOT/ports/lynx/versions.env"
      sha256sum "$MYOS_ROOT/ports/lynx/build.sh"
      sha256sum "$MYOS_ROOT/ports/lynx/prepare.sh"
      sha256sum "$MYOS_ROOT/ports/lynx/fetch.sh"
      # mbedtls is a lynx build dependency. Hash its checkout-stable SOURCE inputs,
      # NOT the post-build target/.myos-mbedtls-version stamp: that stamp exists
      # when lynx builds mbedtls itself but is absent in a downstream ``build`` job
      # that only pulls the artifact, which made the push-pull tag drift and CI
      # fail with ``registry miss lynx: no manifest`` (initramfs missing lynx ELF).
      sha256sum "$MYOS_ROOT/ports/mbedtls/build.sh" "$MYOS_ROOT/ports/mbedtls/fetch.sh" \
        "$MYOS_ROOT/ports/mbedtls/versions.env" "$MYOS_ROOT/ports/mbedtls/myos_mbedtls_config.h" \
        2>/dev/null || true
      find "$MYOS_ROOT/ports/mbedtls/include" -type f -print0 2>/dev/null \
        | sort -z | xargs -0 sha256sum 2>/dev/null || true
      find "$MYOS_ROOT/ports/lynx" -type f -print0 2>/dev/null \
        | sort -z | xargs -0 sha256sum
    } | sha256sum | awk '{print $1}'
  )"
  printf '%s' "$h"
}

myos_lynx_is_current() {
  local arch
  [[ -f "$MYOS_LYNX_VERSION" ]] \
    && [[ "$(cat "$MYOS_LYNX_VERSION")" == "$(myos_lynx_version_hash)" ]] \
    || return 1
  for arch in x86_64 aarch64 riscv64; do
    [[ -f "$MYOS_ROOT/target/lynx-${arch}-unknown-none" ]] || return 1
  done
}

myos_make_version_hash() {
  local h
  h="$(
    {
      myos_newlib_version_hash
      find "$MYOS_ROOT/ports/make" -type f -print0 2>/dev/null \
        | sort -z | xargs -0 sha256sum
    } | sha256sum | awk '{print $1}'
  )"
  printf '%s' "$h"
}

myos_make_is_current() {
  local arch
  [[ -f "$MYOS_MAKE_VERSION" ]] \
    && [[ "$(cat "$MYOS_MAKE_VERSION")" == "$(myos_make_version_hash)" ]] \
    || return 1
  for arch in x86_64 aarch64 riscv64; do
    [[ -f "$MYOS_ROOT/target/make-${arch}-unknown-none" ]] || return 1
  done
}
