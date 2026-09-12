#!/usr/bin/env bash
# Cross-build Lynx with newlib + libgloss sockets + ncurses + mbedtls tidy_tls.
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$HERE/../.." && pwd)"
# shellcheck source=scripts/myos-c-userspace-lib.sh
source "$ROOT/scripts/myos-c-userspace-lib.sh"
# shellcheck source=ports/lynx/versions.env
source "$HERE/versions.env"

if myos_lynx_is_current; then
  echo "lynx ELFs up to date"
  exit 0
fi

"$ROOT/ports/lynx/prepare.sh"
"$ROOT/ports/ncurses/build.sh"
"$ROOT/ports/mbedtls/build.sh"
"$ROOT/toolchain/newlib/build.sh"
# Softfloat helpers for riscv (same as curl).
if [[ -f "$ROOT/ports/curl/build-softfloat-riscv64.sh" ]]; then
  "$ROOT/ports/curl/build-softfloat-riscv64.sh" || true
fi
export PATH="$ROOT/target/newlib-bin:$PATH"

WORK="$ROOT/target/lynx-myos-build"
WWW="$WORK/WWW/Library/Implementation"
SRC="$WORK/src"

# WWW/Library sources (trimmed: skip WAIS / VMS / news when DISABLE_NEWS).
WWW_SRCS=(
  HTAABrow.c HTAAProt.c HTAAUtil.c HTAccess.c HTAnchor.c HTAssoc.c
  HTAtom.c HTBTree.c HTChunk.c HTDOS.c HTFTP.c HTFile.c
  HTFormat.c HTGroup.c HTLex.c HTList.c
  HTMIME.c HTMLDTD.c HTMLGen.c HTParse.c HTPlain.c
  HTRules.c HTString.c HTStyle.c HTTCP.c HTTLS.c HTTP.c
  HTTelnet.c HTUU.c HTWSRC.c SGML.c
)
# Disabled protocols still have stub registration in some units — omit bodies.
# HTFinger / HTGopher / HTNews omitted (DISABLE_*).

# Lynx src (from makefile.in OBJS, minus optional extras).
LYNX_SRCS=(
  LYebcdic.c LYClean.c LYShowInfo.c LYEdit.c LYStrings.c LYMail.c
  HTAlert.c GridText.c LYGetFile.c LYMain.c LYMainLoop.c
  LYCurses.c LYBookmark.c LYmktime.c LYUtils.c LYOptions.c
  LYReadCFG.c LYSearch.c LYHistory.c LYForms.c LYPrint.c
  LYrcFile.c LYDownload.c LYNews.c LYKeymap.c HTML.c
  HTFWriter.c HTInit.c DefaultStyle.c LYUpload.c
  LYLeaks.c LYexit.c LYJump.c LYList.c LYCgi.c
  LYTraversal.c LYEditmap.c LYCharSets.c LYCharUtils.c
  LYMap.c LYCookie.c LYHash.c
  TRSTable.c parsdate.c
  UCdomap.c UCAux.c UCAuto.c
  tidy_tls.c myos_stubs.c
)

link_prog() {
  local arch="$1"
  shift
  local objs=(${@+"$@"})
  local triple="${arch}-unknown-myos"
  local out="$ROOT/target/lynx-${arch}-unknown-none"
  local prefix="$ROOT/target/newlib-${arch}"
  local lib="$prefix/${triple}/lib"
  local nclib="$ROOT/target/ncurses-${arch}/lib"
  local mbed="$ROOT/target/mbedtls-${arch}/lib"
  local ld="${triple}-ld"
  local extra=()

  if [[ "$arch" == "aarch64" ]]; then
    if [[ -f "$ROOT/ports/sbase/trunctfdf2.c" ]]; then
      "${triple}-cc" -ffreestanding -fPIC -O2 -isystem "$prefix/${triple}/include" \
        -c "$ROOT/ports/sbase/trunctfdf2.c" -o "$ROOT/target/lynx-obj-${arch}/trunctfdf2.o"
      extra+=("$ROOT/target/lynx-obj-${arch}/trunctfdf2.o")
    fi
  elif [[ "$arch" == "riscv64" ]]; then
    # Quad/long-double + soft-float shims (riscv64-softfloat.c already has trunc/extend TF).
    "${triple}-cc" -ffreestanding -fPIC -O2 -isystem "$prefix/${triple}/include" \
      -c "$ROOT/ports/sbase/riscv64-softfloat.c" -o "$ROOT/target/lynx-obj-${arch}/riscv64-softfloat.o"
    extra+=("$ROOT/target/lynx-obj-${arch}/riscv64-softfloat.o")
    if [[ ! -f "$ROOT/target/libsoftfloat-riscv64.a" ]]; then
      echo "missing target/libsoftfloat-riscv64.a" >&2
      return 1
    fi
    extra+=("$ROOT/target/libsoftfloat-riscv64.a")
  fi

  echo "  LD lynx ($triple)"
  "$ld" -pie --no-dynamic-linker --gc-sections -o "$out" \
    --entry=_start -z max-page-size=4096 \
    "$lib/crt0.o" "${objs[@]}" "${extra[@]}" \
    -L"$lib" -L"$nclib" -L"$mbed" \
    --start-group -lncurses -lmbedtls -lmbedx509 -lmbedcrypto -lc -lm -lgloss -lg --end-group
  "${triple}-strip" -s "$out" 2>/dev/null || strip -s "$out" 2>/dev/null || true
  ls -lh "$out"
}

build_arch() {
  local arch="$1"
  local triple="${arch}-unknown-myos"
  local prefix="$ROOT/target/newlib-${arch}"
  local inc="$prefix/${triple}/include"
  local ncinc="$ROOT/target/ncurses-${arch}/include"
  local mbedinc="$ROOT/target/mbedtls-${arch}/include"
  local cc="${triple}-cc"
  local objdir="$ROOT/target/lynx-obj-${arch}"
  local clang_res
  clang_res="$(clang -print-resource-dir)/include"
  local objs=()
  local f o
  local cppflags=(
    -DHAVE_CONFIG_H
    -D_DEFAULT_SOURCE
    -D_GNU_SOURCE
    -D_BSD_SOURCE
    -I"$WORK"
    -I"$SRC"
    -I"$SRC/chrtrans"
    -I"$WWW"
    -I"$HERE"
    -I"$ROOT/toolchain/newlib/libgloss/myos"
    -isystem "$ncinc"
    -isystem "$mbedinc"
    -DMBEDTLS_CONFIG_FILE='"myos_mbedtls_config.h"'
    -I"$ROOT/ports/mbedtls"
    -include myos_compat.h
  )
  local cflags=(
    -ffreestanding -fPIC -O2 -std=gnu99
    -ffunction-sections -fdata-sections
    -Wno-unused-parameter -Wno-unused-variable -Wno-unused-function
    -Wno-pointer-sign -Wno-missing-field-initializers
    -Wno-deprecated-declarations -Wno-sign-compare
    -isystem "$clang_res"
    -isystem "$inc"
  )

  rm -rf "$objdir"
  mkdir -p "$objdir/www" "$objdir/src"

  # Ensure termios.h in sysroot (ncurses/vim pattern).
  cp "$ROOT/toolchain/newlib/libgloss/myos/termios.h" "$inc/termios.h"
  # Lynx includes <ncurses.h>; ports/ncurses installs curses.h only.
  ln -sfn curses.h "$ncinc/ncurses.h"

  echo "==> lynx ($triple)"
  for f in "${WWW_SRCS[@]}"; do
    [[ -f "$WWW/$f" ]] || { echo "missing WWW $f" >&2; return 1; }
    o="$objdir/www/$(basename "$f" .c).o"
    if ! "$cc" "${cflags[@]}" "${cppflags[@]}" -c "$WWW/$f" -o "$o" 2>"$objdir/www/$(basename "$f" .c).err"; then
      echo "FAIL www/$f" >&2
      head -20 "$objdir/www/$(basename "$f" .c).err" >&2 || true
      return 1
    fi
    objs+=("$o")
  done

  for f in "${LYNX_SRCS[@]}"; do
    [[ -f "$SRC/$f" ]] || { echo "missing src/$f" >&2; return 1; }
    o="$objdir/src/$(basename "$f" .c).o"
    if ! "$cc" "${cflags[@]}" "${cppflags[@]}" -c "$SRC/$f" -o "$o" 2>"$objdir/src/$(basename "$f" .c).err"; then
      echo "FAIL src/$f" >&2
      head -20 "$objdir/src/$(basename "$f" .c).err" >&2 || true
      return 1
    fi
    objs+=("$o")
  done

  link_prog "$arch" "${objs[@]}"
}

ARCHES="${MYOS_LYNX_ARCHES:-x86_64 aarch64 riscv64}"
for arch in $ARCHES; do
  build_arch "$arch"
done

echo "$(myos_lynx_version_hash)" >"$MYOS_LYNX_VERSION"
echo "lynx build ok ($LYNX_VERSION)"
