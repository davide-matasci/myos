#!/usr/bin/env bash
# Shared version stamps for newlib + C userspace smoke ELFs (CI cache invalidation).
set -euo pipefail

MYOS_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
export MYOS_ROOT

MYOS_NEWLIB_TAG="${NEWLIB_TAG:-newlib-4.4.0}"

# A cache key from the `sha256sum` lines (and version strings) on stdin.
# The checkout path is cut from them first, so a port's key is the same
# wherever the repository is checked out and a local clone pulls what CI
# built (issue #286).
myos_hash() {
  awk -v root="$MYOS_ROOT/" '{ i = index($0, root); if (i) $0 = substr($0, 1, i - 1) substr($0, i + length(root)) } 1' \
    | sha256sum | awk '{print $1}'
}

# The directory of a port: under ports/ (in the image) or packages/ (a
# package, see docs/ports.md); a port moves between the two by moving it.
myos_port_dir() {
  if [[ -d "$MYOS_ROOT/ports/$1" ]]; then
    echo "$MYOS_ROOT/ports/$1"
  else
    echo "$MYOS_ROOT/packages/$1"
  fi
}

MYOS_NEWLIB_VERSION="$MYOS_ROOT/target/.myos-newlib-version"
MYOS_C_HELLO_VERSION="$MYOS_ROOT/target/.myos-c-hello-version"
MYOS_C_SMOKES_VERSION="$MYOS_ROOT/target/.myos-c-smokes-version"
MYOS_CURL_VERSION="$MYOS_ROOT/target/.myos-curl-version"
MYOS_SBASE_VERSION="$MYOS_ROOT/target/.myos-sbase-version"
MYOS_OKSH_VERSION="$MYOS_ROOT/target/.myos-oksh-version"
MYOS_LIMINE_VERSION="$MYOS_ROOT/target/.myos-limine-version"
MYOS_UBASE_VERSION="$MYOS_ROOT/target/.myos-ubase-version"
MYOS_COREUTILS_VERSION="$MYOS_ROOT/target/.myos-coreutils-version"
MYOS_RIPGREP_VERSION="$MYOS_ROOT/target/.myos-ripgrep-version"
MYOS_TCC_VERSION="$MYOS_ROOT/target/.myos-tcc-version"
MYOS_VIM_VERSION="$MYOS_ROOT/target/.myos-vim-version"
MYOS_NCURSES_VERSION="$MYOS_ROOT/target/.myos-ncurses-version"
MYOS_CLEAR_VERSION="$MYOS_ROOT/target/.myos-clear-version"
MYOS_X11_LIBS_VERSION="$MYOS_ROOT/target/.myos-x11-libs-version"
MYOS_TINYX_VERSION="$MYOS_ROOT/target/.myos-tinyx-version"
MYOS_DWM_VERSION="$MYOS_ROOT/target/.myos-dwm-version"
MYOS_ST_VERSION="$MYOS_ROOT/target/.myos-st-version"
MYOS_BOTTOM_VERSION="$MYOS_ROOT/target/.myos-bottom-version"
MYOS_DMENU_VERSION="$MYOS_ROOT/target/.myos-dmenu-version"
MYOS_X11_XFT_VERSION="$MYOS_ROOT/target/.myos-x11-xft-version"
MYOS_X11_APPS_VERSION="$MYOS_ROOT/target/.myos-x11-apps-version"
MYOS_X11_FONTS_VERSION="$MYOS_ROOT/target/.myos-x11-fonts-version"
MYOS_ZLIB_VERSION="$MYOS_ROOT/target/.myos-zlib-version"
MYOS_GIT_VERSION="$MYOS_ROOT/target/.myos-git-version"
MYOS_LYNX_VERSION="$MYOS_ROOT/target/.myos-lynx-version"
MYOS_MAKE_VERSION="$MYOS_ROOT/target/.myos-make-version"
MYOS_LUA_VERSION="$MYOS_ROOT/target/.myos-lua-version"
MYOS_DROPBEAR_VERSION_STAMP="$MYOS_ROOT/target/.myos-dropbear-version"
MYOS_OS_TEST_VERSION="$MYOS_ROOT/target/.myos-os-test-version"
MYOS_LINUX_COMPAT_VERSION="$MYOS_ROOT/target/.myos-linux-compat-version"
MYOS_GET_MYOS_VERSION="$MYOS_ROOT/target/.myos-get-myos-version"

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
    } | myos_hash
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
      sha256sum "$MYOS_ROOT/user/c/hello.c"
      sha256sum "$MYOS_ROOT/user/c/socket_smoke.c"
      sha256sum "$MYOS_ROOT/scripts/build-c-hello.sh"
    } | myos_hash
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

# get-myos (user/get-myos): its sources, the shared pkgtools, the build
# script, and what it links (newlib, zlib).
myos_get_myos_version_hash() {
  local h
  h="$(
    {
      sha256sum "$MYOS_ROOT/user/get-myos/get-myos.c" "$MYOS_ROOT/user/get-myos/pkgtools.c" \
        "$MYOS_ROOT/user/get-myos/pkgtools.h" "$MYOS_ROOT/user/get-myos/boot.c" \
        "$MYOS_ROOT/user/get-myos/boot.h" "$MYOS_ROOT/user/get-myos/build.sh"
      myos_newlib_version_hash
      myos_zlib_version_hash
    } | myos_hash
  )"
  printf '%s' "$h"
}

myos_get_myos_is_current() {
  local arch
  [[ -f "$MYOS_GET_MYOS_VERSION" ]] \
    && [[ "$(cat "$MYOS_GET_MYOS_VERSION")" == "$(myos_get_myos_version_hash)" ]] \
    || return 1
  for arch in x86_64 aarch64 riscv64; do
    [[ -f "$MYOS_ROOT/target/get-myos-${arch}-unknown-none" ]] || return 1
  done
  return 0
}

# All the C smokes (scripts/build-c-smokes.sh): c-hello's inputs plus the
# other smoke sources and their build scripts.
myos_c_smokes_version_hash() {
  local h
  h="$(
    {
      myos_c_hello_version_hash
      sha256sum "$MYOS_ROOT/user/c/tcp_listen_smoke.c" "$MYOS_ROOT/user/c/pty_smoke.c" \
        "$MYOS_ROOT/user/c/urandom_smoke.c" "$MYOS_ROOT/user/c/tty_smoke.c" \
        "$MYOS_ROOT/user/c/unix_smoke.c" "$MYOS_ROOT/user/c/fb_smoke.c" \
        "$MYOS_ROOT/user/c/poll_smoke.c" "$MYOS_ROOT/user/c/kbd_smoke.c" \
        "$MYOS_ROOT/user/c/uio_smoke.c" "$MYOS_ROOT/user/c/pthread_smoke.c" \
        "$MYOS_ROOT/user/c/netconv_smoke.c" "$MYOS_ROOT/user/c/loopback_smoke.c" \
        "$MYOS_ROOT/user/c/child_smoke.c" \
        "$MYOS_ROOT/user/c/libc_smoke.c" "$MYOS_ROOT/user/c/sec.c" "$MYOS_ROOT/user/c/at_smoke.c" \
        "$MYOS_ROOT/user/c/fileio_smoke.c" "$MYOS_ROOT/user/c/mmap_smoke.c" "$MYOS_ROOT/user/c/shm_smoke.c" \
        "$MYOS_ROOT/user/c/fault_smoke.c" "$MYOS_ROOT/user/c/memhog.c" \
        "$MYOS_ROOT/scripts/build-c-smokes.sh"
    } | myos_hash
  )"
  printf '%s' "$h"
}

myos_c_smokes_is_current() {
  local arch bin
  [[ -f "$MYOS_C_SMOKES_VERSION" ]] \
    && [[ "$(cat "$MYOS_C_SMOKES_VERSION")" == "$(myos_c_smokes_version_hash)" ]] \
    || return 1
  for arch in x86_64 aarch64 riscv64; do
    for bin in c-hello c-socket_smoke tcp-listen-smoke pty-smoke urandom-smoke tty-smoke unix-smoke fb-smoke poll-smoke kbd-smoke uio-smoke pthread-smoke netconv-smoke loopback-smoke child-smoke libc-smoke sec at-smoke fileio-smoke mmap-smoke shm-smoke fault-smoke memhog; do
      [[ -f "$MYOS_ROOT/target/${bin}-${arch}-unknown-none" ]] || return 1
    done
  done
  return 0
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
    } | myos_hash
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
    [[ -f "$MYOS_ROOT/target/sbase-${arch}-unknown-none" ]] || return 1
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
    } | myos_hash
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

myos_limine_version_hash() {
  local h
  h="$(
    {
      myos_newlib_version_hash
      find "$MYOS_ROOT/ports/limine" -type f -print0 2>/dev/null \
        | sort -z | xargs -0 sha256sum
    } | myos_hash
  )"
  printf '%s' "$h"
}

myos_limine_is_current() {
  local arch
  [[ -f "$MYOS_LIMINE_VERSION" ]] \
    && [[ "$(cat "$MYOS_LIMINE_VERSION")" == "$(myos_limine_version_hash)" ]] \
    || return 1
  for arch in x86_64 aarch64 riscv64; do
    [[ -f "$MYOS_ROOT/target/limine-${arch}-unknown-none" ]] || return 1
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
    } | myos_hash
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
      sha256sum "$MYOS_ROOT/packages/coreutils/build-uutils.sh"
      sha256sum "$MYOS_ROOT/packages/coreutils/build.sh"
      sha256sum "$MYOS_ROOT/packages/coreutils/prepare.sh"
      sha256sum "$MYOS_ROOT/packages/coreutils/versions.env"
      find "$MYOS_ROOT/packages/coreutils" -type f -print0 2>/dev/null \
        | sort -z | xargs -0 sha256sum
      find "$MYOS_ROOT/ports/crates/libc" -type f -print0 2>/dev/null \
        | sort -z | xargs -0 sha256sum
      sha256sum "$MYOS_ROOT/packages/coreutils/bins.txt"
      sha256sum "$MYOS_ROOT/packages/coreutils/cargo-config.toml"
    } | myos_hash
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
    expected="$(myos_bins_txt_count "$MYOS_ROOT/packages/coreutils/bins.txt")" || return 1
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
    } | myos_hash
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


myos_dropbear_version_hash() {
  if [[ -f "$MYOS_ROOT/ports/dropbear/versions.env" ]]; then
    # shellcheck disable=SC1091
    source "$MYOS_ROOT/ports/dropbear/versions.env"
  fi
  local h
  h="$(
    {
      echo "$MYOS_DROPBEAR_VERSION"
      sha256sum "$MYOS_ROOT/ports/dropbear/build.sh" \
        "$MYOS_ROOT/ports/dropbear/fetch.sh" \
        "$MYOS_ROOT/ports/dropbear/versions.env" \
        "$MYOS_ROOT/ports/dropbear/config-myos.h" \
        "$MYOS_ROOT/ports/dropbear/localoptions.h" \
        "$MYOS_ROOT/ports/dropbear/myos_compat.h" \
        "$MYOS_ROOT/ports/dropbear/prepare.sh" \
        "$MYOS_ROOT/ports/dropbear/myos_shims.c" \
        "$MYOS_ROOT/ports/dropbear/myos_builtins.c" \
        "$MYOS_ROOT"/ports/dropbear/*.myos.patch || true
      myos_newlib_version_hash
    } | myos_hash
  )"
  printf '%s' "$h"
}

myos_dropbear_is_current() {
  local arch
  [[ -f "$MYOS_DROPBEAR_VERSION_STAMP" ]] \
    && [[ "$(cat "$MYOS_DROPBEAR_VERSION_STAMP")" == "$(myos_dropbear_version_hash)" ]] \
    || return 1
  for arch in x86_64 aarch64 riscv64; do
    [[ -f "$MYOS_ROOT/target/dropbear-${arch}-unknown-none" ]] || return 1
    [[ -f "$MYOS_ROOT/target/dbclient-${arch}-unknown-none" ]] || return 1
    [[ -f "$MYOS_ROOT/target/dropbearkey-${arch}-unknown-none" ]] || return 1
  done
  return 0
}

# Linux compatibility layer userspace (linux-compat/build.sh): musl, the
# Linux test binaries and get-alpine. Not the launcher, which has its own
# script and is part of the kernels bundle.
myos_linux_compat_version_hash() {
  local h
  h="$(
    {
      sha256sum "$MYOS_ROOT/linux-compat/build.sh" \
        "$MYOS_ROOT/linux-compat/get-alpine.c" \
        "$MYOS_ROOT/user/get-myos/pkgtools.c" "$MYOS_ROOT/user/get-myos/pkgtools.h" || true
      find "$MYOS_ROOT/linux-compat/tests" -type f -print0 2>/dev/null \
        | sort -z | xargs -0 sha256sum 2>/dev/null || true
      myos_newlib_version_hash
      myos_zlib_version_hash
    } | myos_hash
  )"
  printf '%s' "$h"
}

myos_linux_compat_is_current() {
  local arch f
  [[ -f "$MYOS_LINUX_COMPAT_VERSION" ]] \
    && [[ "$(cat "$MYOS_LINUX_COMPAT_VERSION")" == "$(myos_linux_compat_version_hash)" ]] \
    || return 1
  for arch in x86_64 aarch64 riscv64; do
    [[ -f "$MYOS_ROOT/target/linux-smoke-${arch}-linux-musl" ]] || return 1
    for f in "ld-musl-${arch}.so.1" libsmoke.so libsmoke2.so linux-dyn get-alpine; do
      [[ -f "$MYOS_ROOT/target/linux-compat/${arch}/$f" ]] || return 1
    done
  done
  return 0
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
    } | myos_hash
  )"
  printf '%s' "$h"
}

# The stamp and the three ELFs: what a curl build produces.
myos_curl_elfs_current() {
  local arch
  [[ -f "$MYOS_CURL_VERSION" ]] \
    && [[ "$(cat "$MYOS_CURL_VERSION")" == "$(myos_curl_version_hash)" ]] \
    || return 1
  for arch in x86_64 aarch64 riscv64; do
    [[ -f "$MYOS_ROOT/target/curl-${arch}-unknown-none" ]] || return 1
  done
}

# Every output of the port (PORT_OUTPUTS): the ELFs and the CA bundle the
# build fetches, which the image ships as /lib/cacert.pem. A registry
# package or a workspace without it is not current (the kernel bundle's
# check wants every port file, so a missing bundle rebuilt the kernels in
# every CI run); ports/curl/build.sh fetches it without rebuilding curl.
myos_curl_is_current() {
  myos_curl_elfs_current && [[ -f "$MYOS_ROOT/target/cacert.pem" ]]
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
      sha256sum "$MYOS_ROOT/ports/sbase/riscv64-softfloat.c" 2>/dev/null
    } | myos_hash
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
      sha256sum "$(myos_port_dir vim)/build.sh"
      sha256sum "$(myos_port_dir vim)/prepare.sh"
      sha256sum "$(myos_port_dir vim)/fetch.sh"
      sha256sum "$(myos_port_dir vim)/versions.env"
      find "$(myos_port_dir vim)" -type f -print0 2>/dev/null \
        | sort -z | xargs -0 sha256sum
    } | myos_hash
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
      sha256sum "$(myos_port_dir ncurses)/build.sh"
      sha256sum "$(myos_port_dir ncurses)/prepare.sh"
      sha256sum "$(myos_port_dir ncurses)/fetch.sh"
      sha256sum "$(myos_port_dir ncurses)/versions.env"
      find "$(myos_port_dir ncurses)" -type f -print0 2>/dev/null \
        | sort -z | xargs -0 sha256sum
    } | myos_hash
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

# clear: ncurses' progs/{clear,clear_cmd,tty_settings}.c linked against the
# ncurses library. No own source pin — fold in the ncurses hash, which already
# covers newlib, the ncurses source and its build.
myos_clear_version_hash() {
  local h
  h="$(
    {
      myos_ncurses_version_hash
      find "$(myos_port_dir clear)" -type f -print0 2>/dev/null \
        | sort -z | xargs -0 sha256sum
    } | myos_hash
  )"
  printf '%s' "$h"
}

myos_clear_is_current() {
  local arch
  [[ -f "$MYOS_CLEAR_VERSION" ]] \
    && [[ "$(cat "$MYOS_CLEAR_VERSION")" == "$(myos_clear_version_hash)" ]] \
    || return 1
  for arch in x86_64 aarch64 riscv64; do
    [[ -f "$MYOS_ROOT/target/clear-${arch}-unknown-none" ]] || return 1
  done
}


# The soft-float runtime of riscv64 (no F or D extension): compiler-rt's
# builtins for the double and float arithmetic, compares and integer
# conversions (ports/curl/build-softfloat-riscv64.sh), printed as the
# archive to link after the long-double helpers of
# ports/sbase/riscv64-softfloat.c. Every riscv64 link needs both: newlib's
# printf and strtod call them.
myos_riscv64_softfloat() {
  "$MYOS_ROOT/ports/curl/build-softfloat-riscv64.sh" >/dev/null
  echo "$MYOS_ROOT/target/libsoftfloat-riscv64.a"
}

# myos_write_cross_cc ARCH OUT [CFLAG...]: write OUT, a cc for autoconf
# ports: clang against the newlib sysroot with the CFLAGs, and for a link
# ld.lld with crt0, libc and libgloss the way scripts/build-c-smokes.sh
# links, so configure's link tests answer for myos (--build and --host
# differing keeps configure from running what it links). Not clang's own
# link: for a bare-metal target it hands it to the host's gcc on some
# triples and versions.
myos_write_cross_cc() {
  local arch="$1" out="$2"
  shift 2
  local elf="${arch}-unknown-none"
  local sysroot="$MYOS_ROOT/target/newlib-${arch}/${arch}-unknown-myos"
  local clanginc extra="" flags="" f
  clanginc="$(clang -print-resource-dir)/include"
  for f in "$@"; do
    flags="$flags $(printf '%q' "$f")"
  done
  # newlib's printf wants the long-double helpers these arches lack (the
  # sbase port carries them); riscv64 has no FPU, so its float and double
  # arithmetic, conversions and compares are compiler-rt's as well.
  case "$arch" in
    aarch64) extra="$out.helpers.o"
      clang --target="$elf" -ffreestanding -fPIC -O2 -isystem "$sysroot/include" \
        -c "$MYOS_ROOT/ports/sbase/trunctfdf2.c" -o "$extra" ;;
    riscv64)
      clang --target="$elf" -ffreestanding -fPIC -O2 -isystem "$sysroot/include" \
        -c "$MYOS_ROOT/ports/sbase/riscv64-softfloat.c" -o "$out.helpers.o"
      extra="$out.helpers.o $(myos_riscv64_softfloat)" ;;
  esac
  cat > "$out" <<EOC
#!/usr/bin/env bash
cflags=(--target=$elf -ffreestanding -fPIC -nostdinc -isystem $clanginc -isystem $sysroot/include$flags)
sysroot=$sysroot
extra="$extra"
EOC
  cat >> "$out" <<'EOC'
for a in "$@"; do
  case "$a" in -c|-E|-S|-M|-MM) exec clang "${cflags[@]}" "$@" ;; esac
done
# Linking: compile what is C here, then ld.lld, keeping the order of the
# objects, -L and -l.
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
out=a.out
flags=() srcs=() inputs=()
while [ $# -gt 0 ]; do
  case "$1" in
    -o) out="$2"; shift ;;
    -include|-isystem|-I|-D|-U|-x|-MF|-MT|-MQ) flags+=("$1" "$2"); shift ;;
    *.c) srcs+=("$1"); inputs+=("$tmp/${#srcs[@]}.o") ;;
    -Wl,*) IFS=, read -ra w <<< "${1#-Wl,}"; inputs+=("${w[@]}") ;;
    -Xlinker) inputs+=("$2"); shift ;;
    -L*|-l*) inputs+=("$1") ;;
    -pthread|-static|-rdynamic) ;;
    -*) flags+=("$1") ;;
    *) inputs+=("$1") ;;
  esac
  shift
done
for i in "${!srcs[@]}"; do
  clang "${cflags[@]}" "${flags[@]}" -c "${srcs[$i]}" -o "$tmp/$((i + 1)).o" || exit 1
done
exec ld.lld -pie --no-dynamic-linker --entry=_start -z max-page-size=4096 -o "$out" \
  "$sysroot/lib/crt0.o" "${inputs[@]}" $extra \
  -L"$sysroot/lib" --start-group -lc -lgloss -lg --end-group
EOC
  chmod +x "$out"
}

myos_x11_libs_version_hash() {
  local h
  h="$(
    {
      myos_newlib_version_hash
      find "$(myos_port_dir x11-libs)" -type f -print0 2>/dev/null \
        | sort -z | xargs -0 sha256sum
    } | myos_hash
  )"
  printf '%s' "$h"
}

myos_x11_libs_is_current() {
  local arch
  [[ -f "$MYOS_X11_LIBS_VERSION" ]] \
    && [[ "$(cat "$MYOS_X11_LIBS_VERSION")" == "$(myos_x11_libs_version_hash)" ]] \
    || return 1
  for arch in x86_64 aarch64 riscv64; do
    [[ -f "$MYOS_ROOT/target/x11-libs-${arch}/lib/x11/lib/libX11.a" ]] || return 1
    [[ -f "$MYOS_ROOT/target/x11-smoke-${arch}-unknown-none" ]] || return 1
  done
}

myos_tinyx_version_hash() {
  local h
  h="$(
    {
      myos_newlib_version_hash
      myos_x11_libs_version_hash
      myos_zlib_version_hash
      find "$(myos_port_dir tinyx)" -type f -print0 2>/dev/null \
        | sort -z | xargs -0 sha256sum
    } | myos_hash
  )"
  printf '%s' "$h"
}

myos_tinyx_is_current() {
  local arch
  [[ -f "$MYOS_TINYX_VERSION" ]] \
    && [[ "$(cat "$MYOS_TINYX_VERSION")" == "$(myos_tinyx_version_hash)" ]] \
    || return 1
  for arch in x86_64 aarch64 riscv64; do
    [[ -f "$MYOS_ROOT/target/xfbdev-${arch}-unknown-none" ]] || return 1
    [[ -f "$MYOS_ROOT/target/tinyx-smoke-${arch}-unknown-none" ]] || return 1
    [[ -f "$MYOS_ROOT/target/startx-${arch}-unknown-none" ]] || return 1
  done
}

myos_x11_xft_version_hash() {
  local h
  h="$(
    {
      myos_newlib_version_hash
      myos_x11_libs_version_hash
      find "$(myos_port_dir x11-xft)" -type f -print0 2>/dev/null \
        | sort -z | xargs -0 sha256sum
    } | myos_hash
  )"
  printf '%s' "$h"
}

myos_x11_xft_is_current() {
  local arch
  [[ -f "$MYOS_X11_XFT_VERSION" ]] \
    && [[ "$(cat "$MYOS_X11_XFT_VERSION")" == "$(myos_x11_xft_version_hash)" ]] \
    || return 1
  for arch in x86_64 aarch64 riscv64; do
    [[ -f "$MYOS_ROOT/target/x11-xft-${arch}/lib/x11/lib/libXft.a" ]] || return 1
    [[ -f "$MYOS_ROOT/target/xft-smoke-${arch}-unknown-none" ]] || return 1
    [[ -f "$MYOS_ROOT/target/fc-match-${arch}-unknown-none" ]] || return 1
  done
}

myos_x11_fonts_version_hash() {
  local h
  h="$(
    find "$(myos_port_dir x11-fonts)" -type f -print0 2>/dev/null \
      | sort -z | xargs -0 sha256sum | myos_hash
  )"
  printf '%s' "$h"
}

myos_x11_fonts_is_current() {
  [[ -f "$MYOS_X11_FONTS_VERSION" ]] \
    && [[ "$(cat "$MYOS_X11_FONTS_VERSION")" == "$(myos_x11_fonts_version_hash)" ]] \
    && [[ -f "$MYOS_ROOT/target/x11-fonts/DejaVuSansMono.ttf" ]]
}

myos_dwm_version_hash() {
  local h
  h="$(
    {
      myos_newlib_version_hash
      myos_x11_xft_version_hash
      find "$(myos_port_dir dwm)" -type f -print0 2>/dev/null \
        | sort -z | xargs -0 sha256sum
    } | myos_hash
  )"
  printf '%s' "$h"
}

myos_dwm_is_current() {
  local arch
  [[ -f "$MYOS_DWM_VERSION" ]] \
    && [[ "$(cat "$MYOS_DWM_VERSION")" == "$(myos_dwm_version_hash)" ]] \
    || return 1
  for arch in x86_64 aarch64 riscv64; do
    [[ -f "$MYOS_ROOT/target/dwm-${arch}-unknown-none" ]] || return 1
    [[ -f "$MYOS_ROOT/target/dwm-smoke-${arch}-unknown-none" ]] || return 1
  done
}

myos_st_version_hash() {
  local h
  h="$(
    {
      myos_newlib_version_hash
      myos_x11_xft_version_hash
      find "$(myos_port_dir st)" -type f -print0 2>/dev/null \
        | sort -z | xargs -0 sha256sum
    } | myos_hash
  )"
  printf '%s' "$h"
}

myos_st_is_current() {
  local arch
  [[ -f "$MYOS_ST_VERSION" ]] \
    && [[ "$(cat "$MYOS_ST_VERSION")" == "$(myos_st_version_hash)" ]] \
    || return 1
  for arch in x86_64 aarch64 riscv64; do
    [[ -f "$MYOS_ROOT/target/st-${arch}-unknown-none" ]] || return 1
    [[ -f "$MYOS_ROOT/target/st-smoke-${arch}-unknown-none" ]] || return 1
  done
}

myos_bottom_version_hash() {
  local h
  h="$(
    {
      # Rust: the std sysroot, and the myos libc/errno/rustix crates the
      # Rust ports share (packages/coreutils/prepare.sh); C: newlib (btm_smoke,
      # and libgloss behind the libc crate).
      myos_sysroot_version_hash
      myos_newlib_version_hash
      find "$(myos_port_dir bottom)" "$MYOS_ROOT/ports/crates/libc" \
        "$MYOS_ROOT/packages/coreutils/crates" -type f -print0 2>/dev/null \
        | sort -z | xargs -0 sha256sum
      sha256sum "$MYOS_ROOT/packages/coreutils/prepare.sh" "$MYOS_ROOT/packages/coreutils/versions.env"
    } | myos_hash
  )"
  printf '%s' "$h"
}

myos_bottom_is_current() {
  local arch
  [[ -f "$MYOS_BOTTOM_VERSION" ]] \
    && [[ "$(cat "$MYOS_BOTTOM_VERSION")" == "$(myos_bottom_version_hash)" ]] \
    || return 1
  for arch in x86_64 aarch64 riscv64; do
    [[ -f "$MYOS_ROOT/target/btm-${arch}-unknown-myos" ]] || return 1
    [[ -f "$MYOS_ROOT/target/btm-smoke-${arch}-unknown-none" ]] || return 1
  done
}

myos_dmenu_version_hash() {
  local h
  h="$(
    {
      myos_newlib_version_hash
      myos_x11_xft_version_hash
      find "$(myos_port_dir dmenu)" -type f -print0 2>/dev/null \
        | sort -z | xargs -0 sha256sum
    } | myos_hash
  )"
  printf '%s' "$h"
}

myos_dmenu_is_current() {
  local arch
  [[ -f "$MYOS_DMENU_VERSION" ]] \
    && [[ "$(cat "$MYOS_DMENU_VERSION")" == "$(myos_dmenu_version_hash)" ]] \
    || return 1
  for arch in x86_64 aarch64 riscv64; do
    [[ -f "$MYOS_ROOT/target/dmenu-${arch}-unknown-none" ]] || return 1
    [[ -f "$MYOS_ROOT/target/stest-${arch}-unknown-none" ]] || return 1
    [[ -f "$MYOS_ROOT/target/dmenu-smoke-${arch}-unknown-none" ]] || return 1
  done
  [[ -f "$MYOS_ROOT/target/dmenu_run" && -f "$MYOS_ROOT/target/dmenu_path" ]]
}

myos_x11_apps_version_hash() {
  local h
  h="$(
    {
      myos_newlib_version_hash
      myos_x11_xft_version_hash
      find "$(myos_port_dir x11-apps)" -type f -print0 2>/dev/null \
        | sort -z | xargs -0 sha256sum
    } | myos_hash
  )"
  printf '%s' "$h"
}

myos_x11_apps_is_current() {
  local arch
  [[ -f "$MYOS_X11_APPS_VERSION" ]] \
    && [[ "$(cat "$MYOS_X11_APPS_VERSION")" == "$(myos_x11_apps_version_hash)" ]] \
    || return 1
  for arch in x86_64 aarch64 riscv64; do
    [[ -f "$MYOS_ROOT/target/xev-${arch}-unknown-none" ]] || return 1
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
    } | myos_hash
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
      sha256sum "$(myos_port_dir git)/build.sh"
      sha256sum "$(myos_port_dir git)/prepare.sh"
      sha256sum "$(myos_port_dir git)/fetch.sh"
      sha256sum "$(myos_port_dir git)/versions.env"
      find "$(myos_port_dir git)" -type f -print0 2>/dev/null \
        | sort -z | xargs -0 sha256sum
    } | myos_hash
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
      sha256sum "$(myos_port_dir lynx)/versions.env"
      sha256sum "$(myos_port_dir lynx)/build.sh"
      sha256sum "$(myos_port_dir lynx)/prepare.sh"
      sha256sum "$(myos_port_dir lynx)/fetch.sh"
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
      find "$(myos_port_dir lynx)" -type f -print0 2>/dev/null \
        | sort -z | xargs -0 sha256sum
    } | myos_hash
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

myos_lua_version_hash() {
  local h
  h="$(
    {
      myos_newlib_version_hash
      sha256sum "$(myos_port_dir lua)/versions.env"
      sha256sum "$(myos_port_dir lua)/build.sh"
      sha256sum "$(myos_port_dir lua)/fetch.sh"
      find "$(myos_port_dir lua)" -type f -print0 2>/dev/null \
        | sort -z | xargs -0 sha256sum
    } | myos_hash
  )"
  printf '%s' "$h"
}

myos_lua_is_current() {
  local arch
  [[ -f "$MYOS_LUA_VERSION" ]] \
    && [[ "$(cat "$MYOS_LUA_VERSION")" == "$(myos_lua_version_hash)" ]] \
    || return 1
  for arch in x86_64 aarch64 riscv64; do
    [[ -f "$MYOS_ROOT/target/lua-${arch}-unknown-none" ]] || return 1
  done
}

myos_make_version_hash() {
  local h
  h="$(
    {
      myos_newlib_version_hash
      find "$(myos_port_dir make)" -type f -print0 2>/dev/null \
        | sort -z | xargs -0 sha256sum
      sha256sum "$MYOS_ROOT/ports/sbase/riscv64-softfloat.c" 2>/dev/null
    } | myos_hash
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

myos_os_test_version_hash() {
  local h
  h="$(
    {
      myos_newlib_version_hash
      sha256sum "$(myos_port_dir os-test)/versions.env"
      sha256sum "$(myos_port_dir os-test)/build.sh"
      sha256sum "$(myos_port_dir os-test)/fetch.sh"
      sha256sum "$(myos_port_dir os-test)/prebuild-basic-smoke.sh"
      find "$(myos_port_dir os-test)/overlay" -type f -print0 2>/dev/null \
        | sort -z | xargs -0 sha256sum
      sha256sum "$MYOS_ROOT/ports/sbase/trunctfdf2.c" 2>/dev/null
      sha256sum "$MYOS_ROOT/ports/sbase/riscv64-softfloat.c" 2>/dev/null
    } | myos_hash
  )"
  printf '%s' "$h"
}

myos_os_test_is_current() {
  local arch marker
  [[ -f "$MYOS_OS_TEST_VERSION" ]] \
    && [[ "$(cat "$MYOS_OS_TEST_VERSION")" == "$(myos_os_test_version_hash)" ]] \
    || return 1
  [[ -f "$MYOS_ROOT/target/os-test-embed/basic/ctype/isalnum.c" ]] || return 1
  for arch in x86_64 aarch64 riscv64; do
    marker="$MYOS_ROOT/target/os-test-prebuilt/${arch}/basic/arpa_inet/htons"
    [[ -f "$marker" ]] || return 1
  done
}
