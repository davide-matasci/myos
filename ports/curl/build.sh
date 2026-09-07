#!/usr/bin/env bash
# Cross-build trimmed curl (HTTPS GET + -o) with userspace sockets + mbedtls.
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$HERE/../.." && pwd)"
# shellcheck source=versions.env
source "$HERE/versions.env"
# shellcheck source=scripts/myos-c-userspace-lib.sh
source "$ROOT/scripts/myos-c-userspace-lib.sh"

STAMP="$ROOT/target/.myos-curl-version"
hash_curl() {
  {
    echo "$CURL_VERSION"
    sha256sum "$HERE/build.sh" "$HERE/fetch.sh" "$HERE/versions.env" "$HERE/config-myos.h" || true
    # Statically links mbedtls: rebuild when CA/FS config changes.
    sha256sum "$ROOT/ports/mbedtls/myos_mbedtls_config.h" || true
    if [[ -f "$ROOT/target/.myos-mbedtls-version" ]]; then
      sha256sum "$ROOT/target/.myos-mbedtls-version" || true
    fi
    myos_newlib_version_hash
  } | sha256sum | awk '{print $1}'
}
WANT="$(hash_curl)"

pack_curl_aliases() {
  # CI packs via existing `target/coreutils-*` glob (workflow edits need workflow scope).
  # Keep canonical curl-* and pack-alias coreutils-curl-* in sync either way so
  # aarch64/riscv initramfs does not log "skip curl-*" when only the alias landed.
  local arch src alias
  for arch in x86_64 aarch64 riscv64; do
    src="$ROOT/target/curl-${arch}-unknown-none"
    alias="$ROOT/target/coreutils-curl-${arch}-unknown-none"
    if [[ -f "$src" ]]; then
      cp "$src" "$alias"
    elif [[ -f "$alias" ]]; then
      cp "$alias" "$src"
    fi
  done
}

need=0
for arch in x86_64 aarch64 riscv64; do
  [[ -f "$ROOT/target/curl-${arch}-unknown-none" ]] || need=1
done
if [[ -f "$STAMP" && "$(cat "$STAMP")" == "$WANT" && "$need" -eq 0 ]]; then
  echo "curl ELFs up to date"
  pack_curl_aliases
  exit 0
fi

"$HERE/fetch.sh"
"$ROOT/toolchain/newlib/build.sh"
"$ROOT/ports/mbedtls/build.sh"
"$HERE/build-softfloat-riscv64.sh"
export PATH="$ROOT/target/newlib-bin:$PATH"

SRC="$ROOT/target/curl-src"
cp "$HERE/config-myos.h" "$SRC/lib/curl_config.h"
# tool_cfgable.h uses curlx_dynbuf without including dynbuf.h (curlx.h omits it).
if ! grep -q 'dynbuf.h' "$SRC/src/tool_cfgable.h"; then
  sed -i '/#include "tool_setup.h"/a#include "dynbuf.h"' "$SRC/src/tool_cfgable.h" || true
fi
# Fix curl checking the *value* macro as an ifdef (always true in mbedtls 3.6 headers).
# curl checks the *value* macro as #ifdef (always true in mbedtls 3.6 headers).
sed -i 's/#ifdef MBEDTLS_SSL_TLS1_3_SIGNAL_NEW_SESSION_TICKETS_ENABLED/#if defined(MBEDTLS_SSL_PROTO_TLS1_3) \&\& defined(MBEDTLS_SSL_SESSION_TICKETS)/'   "$SRC/lib/vtls/mbedtls.c" || true

# Expand CSOURCES from Makefile.inc
mapfile -t LIB_SRCS < <(python3 - "$SRC/lib/Makefile.inc" <<'PY'
import re, sys
text = open(sys.argv[1]).read()
vars = {}
for key, val in re.findall(r'^([A-Z0-9_]+)\s*=\s*((?:.*\\\n)*.*)', text, re.M):
    vars[key] = val.replace('\\\n', ' ')
def expand(s, depth=0):
    if depth > 12:
        return s
    return re.sub(r'\$\(([A-Z0-9_]+)\)', lambda m: expand(vars.get(m.group(1), ''), depth+1), s)
for f in expand(vars['CSOURCES']).split():
    if f.endswith('.c'):
        print(f)
PY
)

skip_src() {
  case "$1" in
    vtls/openssl.c|vtls/wolfssl.c|vtls/gtls.c|vtls/bearssl.c|vtls/rustls.c|vtls/schannel.c|vtls/schannel_verify.c|vtls/sectransp.c) return 0 ;;
    vquic/curl_*.c|vquic/vquic-tls.c|vssh/*) return 0 ;;
    vauth/ntlm*|vauth/spnego*|vauth/krb5*|vauth/gsasl.c|vauth/digest_sspi.c|vauth/cram.c|vauth/digest.c|vauth/oauth2.c|vauth/cleartext.c) return 0 ;;
    asyn-ares.c|asyn-thread.c|c-hyper.c|amigaos.c|system_win32.c|version_win32.c) return 0 ;;
    *) return 1 ;;
  esac
}

build_arch() {
  local arch="$1"
  local triple="${arch}-unknown-myos"
  local prefix="$ROOT/target/newlib-${arch}"
  local inc="$prefix/${triple}/include"
  local lib="$prefix/${triple}/lib"
  local cc="${triple}-cc"
  local mbed="$ROOT/target/mbedtls-${arch}"
  local objdir="$ROOT/target/curl-build-${arch}"
  local out="$ROOT/target/curl-${arch}-unknown-none"
  local clang_res
  clang_res="$(clang -print-resource-dir)/include"

  echo "==> curl ($triple)"
  rm -rf "$objdir"
  mkdir -p "$objdir/tool"

  local cflags=(
    -ffreestanding -fPIC -Os -g0
    -DHAVE_CONFIG_H
    -DBUILDING_LIBCURL
    -DCURL_STATICLIB
    -DHTTP_ONLY
    -isystem "$clang_res"
    -isystem "$inc"
    -I"$SRC/include"
    -I"$SRC/lib"
    -I"$mbed/include"
    -DMBEDTLS_CONFIG_FILE='"myos_mbedtls_config.h"'
    -I"$ROOT/ports/mbedtls"
    -Wno-unused-parameter
    -Wno-sign-compare
    -Wno-unused-function
    -Wno-deprecated-declarations
  )

  local objs=()
  local f o
  local compiled=0 skipped=0 failed=0
  for f in "${LIB_SRCS[@]}"; do
    if skip_src "$f"; then
      skipped=$((skipped+1))
      continue
    fi
    [[ -f "$SRC/lib/$f" ]] || continue
    o="$objdir/$(echo "$f" | tr '/' '_').o"
    if "$cc" "${cflags[@]}" -c "$SRC/lib/$f" -o "$o" 2>"$objdir/$(echo "$f" | tr '/' '_').err"; then
      objs+=("$o")
      compiled=$((compiled+1))
    else
      failed=$((failed+1))
      echo "FAIL lib $f" >&2
      head -8 "$objdir/$(echo "$f" | tr '/' '_').err" >&2 || true
      return 1
    fi
  done
  echo "  lib: compiled=$compiled skipped=$skipped"

  # tool: parse CURL_CFILES
  mapfile -t TOOL_SRCS < <(python3 - "$SRC/src/Makefile.inc" <<'PY'
import re, sys
text = open(sys.argv[1]).read()
vars = {}
for key, val in re.findall(r'^([A-Z0-9_]+)\s*=\s*((?:.*\\\n)*.*)', text, re.M):
    vars[key] = val.replace('\\\n', ' ')
def expand(s, depth=0):
    if depth > 12: return s
    return re.sub(r'\$\(([A-Z0-9_]+)\)', lambda m: expand(vars.get(m.group(1), ''), depth+1), s)
for f in expand(vars.get('CURL_CFILES','')).split():
    if f.endswith('.c'):
        print(f)
PY
)

  # Provide empty hugehelp if missing
  if [[ ! -f "$SRC/src/tool_hugehelp.c" ]]; then
    printf 'const char *curl_hugehelp = "";\n' >"$SRC/src/tool_hugehelp.c"
  fi

  local tool_objs=()
  local tool_cflags=("${cflags[@]}")
  # tool is not BUILDING_LIBCURL
  local tcflags=()
  local x
  for x in "${cflags[@]}"; do
    [[ "$x" == "-DBUILDING_LIBCURL" ]] && continue
    tcflags+=("$x")
  done
  tcflags+=(-I"$SRC/src" -UBUILDING_LIBCURL)

  for f in "${TOOL_SRCS[@]}"; do
    local path="$SRC/src/$f"
    [[ -f "$path" ]] || continue
    o="$objdir/tool/$(basename "$f" .c).o"
    if "$cc" "${tcflags[@]}" -c "$path" -o "$o" 2>"$objdir/tool/$(basename "$f").err"; then
      tool_objs+=("$o")
    else
      echo "FAIL tool $f" >&2
      head -10 "$objdir/tool/$(basename "$f").err" >&2 || true
      return 1
    fi
  done
  # CURLX compiled without BUILDING_LIBCURL → curlx_* symbols (lib has Curl_*).
  for f in base64.c dynbuf.c; do
    [[ -f "$SRC/lib/$f" ]] || continue
    o="$objdir/tool/curlx_${f%.c}.o"
    if "$cc" "${tcflags[@]}" -c "$SRC/lib/$f" -o "$o" 2>"$objdir/tool/curlx_${f%.c}.err"; then
      tool_objs+=("$o")
    else
      echo "FAIL curlx $f" >&2
      head -5 "$objdir/tool/curlx_${f%.c}.err" >&2 || true
      return 1
    fi
  done
  echo "  tool: ${#tool_objs[@]} objs"

  local extra=()
  if [[ "$arch" == "aarch64" || "$arch" == "riscv64" ]]; then
    local tf="$objdir/trunctfdf2.o"
    "$cc" -ffreestanding -fPIC -O2 -isystem "$inc" -c "$ROOT/ports/sbase/trunctfdf2.c" -o "$tf"
    extra+=("$tf")
  fi
  # Soft-float helpers for riscv (printf/strtod path in curl).
  if [[ "$arch" == "riscv64" ]]; then
    if [[ ! -f "$ROOT/target/libsoftfloat-riscv64.a" ]]; then
      echo "missing target/libsoftfloat-riscv64.a (build compiler-rt softfloat)" >&2
      return 1
    fi
    extra+=("$ROOT/target/libsoftfloat-riscv64.a")
  fi

  # mbedtls entropy/time glue
  o="$objdir/myos_curl_platform.o"
  "$cc" "${cflags[@]}" -c "$HERE/myos_curl_platform.c" -o "$o"
  objs+=("$o")

  echo "  LD curl"
  ld.lld -pie --no-dynamic-linker -o "$out" \
    --entry=_start -z max-page-size=4096 \
    "$lib/crt0.o" "${tool_objs[@]}" "${objs[@]}" "${extra[@]}" \
    -L"$lib" -L"$mbed/lib" \
    --start-group -lmbedtls -lmbedx509 -lmbedcrypto -lc -lgloss -lg --end-group

  "${triple}-strip" -s "$out" 2>/dev/null || strip -s "$out" 2>/dev/null || true
  ls -lh "$out"
}

build_arch x86_64
build_arch aarch64
build_arch riscv64

echo "$WANT" >"$STAMP"
pack_curl_aliases
echo "curl build ok"
