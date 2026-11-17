#!/bin/sh
# Cross-build Lynx with newlib + libgloss sockets + ncurses + LibreSSL.
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
ROOT="$(cd "$HERE/../.." && pwd)"
# shellcheck source=scripts/myos-c-userspace-lib.sh
. "$ROOT/scripts/myos-c-userspace-lib.sh"
# shellcheck source=ports/lynx/versions.env
. "$HERE/versions.env"

if myos_lynx_is_current; then
  echo "lynx ELFs up to date"
  exit 0
fi

"$HERE/prepare.sh"
"$(myos_port_dir ncurses)/build.sh"
"$ROOT/ports/libressl/build.sh"
"$ROOT/toolchain/newlib/build.sh"
export PATH="$ROOT/target/newlib-bin:$PATH"

WORK="$ROOT/target/lynx-myos-build"
WWW="$WORK/WWW/Library/Implementation"
SRC="$WORK/src"

# WWW/Library sources (trimmed: skip WAIS / VMS / news when DISABLE_NEWS).
WWW_SRCS="
  HTAABrow.c HTAAProt.c HTAAUtil.c HTAccess.c HTAnchor.c HTAssoc.c
  HTAtom.c HTBTree.c HTChunk.c HTDOS.c HTFTP.c HTFile.c
  HTFormat.c HTGroup.c HTLex.c HTList.c
  HTMIME.c HTMLDTD.c HTMLGen.c HTParse.c HTPlain.c
  HTRules.c HTString.c HTStyle.c HTTCP.c HTTLS.c HTTP.c
  HTTelnet.c HTUU.c HTWSRC.c SGML.c
"
# Disabled protocols still have stub registration in some units — omit bodies.
# HTFinger / HTGopher / HTNews omitted (DISABLE_*).

# Lynx src (from makefile.in OBJS, minus optional extras).
LYNX_SRCS="
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
  myos_stubs.c
"

link_prog() (
  arch="$1"
  shift
  triple="${arch}-unknown-myos"
  out="$ROOT/target/lynx-${arch}-unknown-none"
  prefix="$ROOT/target/newlib-${arch}"
  lib="$prefix/${triple}/lib"
  nclib="$ROOT/target/ncurses-${arch}/lib"
  tlslib="$ROOT/target/libressl-${arch}/lib"
  ld="${triple}-ld"

  if [ "$arch" = "aarch64" ] || [ "$arch" = "riscv64" ]; then
    if [ -f "$ROOT/ports/sbase/trunctfdf2.c" ]; then
      "${triple}-cc" -ffreestanding -fPIC -O2 -isystem "$prefix/${triple}/include" \
        -c "$ROOT/ports/sbase/trunctfdf2.c" -o "$ROOT/target/lynx-obj-${arch}/trunctfdf2.o"
      set -- "$@" "$ROOT/target/lynx-obj-${arch}/trunctfdf2.o"
    fi
  fi

  echo "  LD lynx ($triple)"
  "$ld" -pie --no-dynamic-linker --gc-sections -o "$out" \
    --entry=_start -z max-page-size=4096 \
    "$lib/crt0.o" "$@" \
    -L"$lib" -L"$nclib" -L"$tlslib" \
    --start-group -lncurses -lssl -lcrypto -lc -lm -lgloss -lg --end-group
  "${triple}-strip" -s "$out" 2>/dev/null || strip -s "$out" 2>/dev/null || true
  ls -lh "$out"
)

# lynx_cc ARG...: build_arch's compiler with its flags (the C ones, then
# the preprocessor's) before the ARGs.
lynx_cc() {
  "$cc" \
    -ffreestanding -fPIC -O2 -std=gnu99 \
    -ffunction-sections -fdata-sections \
    -Wno-unused-parameter -Wno-unused-variable -Wno-unused-function \
    -Wno-pointer-sign -Wno-missing-field-initializers \
    -Wno-deprecated-declarations -Wno-sign-compare \
    -isystem "$clang_res" \
    -isystem "$inc" \
    -DHAVE_CONFIG_H \
    -D_DEFAULT_SOURCE \
    -D_GNU_SOURCE \
    -D_BSD_SOURCE \
    -I"$WORK" \
    -I"$SRC" \
    -I"$SRC/chrtrans" \
    -I"$WWW" \
    -I"$HERE" \
    -I"$ROOT/toolchain/newlib/libgloss/myos" \
    -isystem "$ncinc" \
    -isystem "$tlsinc" \
    -include myos_compat.h \
    "$@"
}

build_arch() (
  arch="$1"
  triple="${arch}-unknown-myos"
  prefix="$ROOT/target/newlib-${arch}"
  inc="$prefix/${triple}/include"
  ncinc="$ROOT/target/ncurses-${arch}/include"
  tlsinc="$ROOT/target/libressl-${arch}/include"
  cc="${triple}-cc"
  objdir="$ROOT/target/lynx-obj-${arch}"
  clang_res="$(clang -print-resource-dir)/include"
  set --

  rm -rf "$objdir"
  mkdir -p "$objdir/www" "$objdir/src"

  # Ensure termios.h in sysroot (ncurses/vim pattern).
  cp "$ROOT/toolchain/newlib/libgloss/myos/termios.h" "$inc/termios.h"
  # Lynx includes <ncurses.h>; ports/ncurses installs curses.h only.
  ln -sfn curses.h "$ncinc/ncurses.h"

  echo "==> lynx ($triple)"
  for f in $WWW_SRCS; do
    [ -f "$WWW/$f" ] || { echo "missing WWW $f" >&2; return 1; }
    o="$objdir/www/$(basename "$f" .c).o"
    if ! lynx_cc -c "$WWW/$f" -o "$o" 2>"$objdir/www/$(basename "$f" .c).err"; then
      echo "FAIL www/$f" >&2
      head -20 "$objdir/www/$(basename "$f" .c).err" >&2 || true
      return 1
    fi
    set -- "$@" "$o"
  done

  for f in $LYNX_SRCS; do
    [ -f "$SRC/$f" ] || { echo "missing src/$f" >&2; return 1; }
    o="$objdir/src/$(basename "$f" .c).o"
    if ! lynx_cc -c "$SRC/$f" -o "$o" 2>"$objdir/src/$(basename "$f" .c).err"; then
      echo "FAIL src/$f" >&2
      head -20 "$objdir/src/$(basename "$f" .c).err" >&2 || true
      return 1
    fi
    set -- "$@" "$o"
  done

  link_prog "$arch" "$@"
)

ARCHES="${MYOS_LYNX_ARCHES:-x86_64 aarch64 riscv64}"
for arch in $ARCHES; do
  build_arch "$arch"
done

echo "$(myos_lynx_version_hash)" >"$MYOS_LYNX_VERSION"
echo "lynx build ok ($LYNX_VERSION)"
