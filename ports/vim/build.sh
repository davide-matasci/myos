#!/usr/bin/env bash
# Cross-build Vim (FEAT_TINY) with newlib + myos libgloss + ncurses termcap.
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$HERE/../.." && pwd)"
# shellcheck source=scripts/myos-c-userspace-lib.sh
source "$ROOT/scripts/myos-c-userspace-lib.sh"

if myos_vim_is_current; then
  echo "vim ELFs up to date"
  exit 0
fi

"$ROOT/ports/vim/prepare.sh"
"$ROOT/ports/ncurses/build.sh"
"$ROOT/toolchain/newlib/build.sh"
export PATH="$ROOT/target/newlib-bin:$PATH"

WORK="$ROOT/target/vim-myos-build/src"
MYOS="$ROOT/ports/vim"

# BASIC_SRC from upstream Makefile (FEAT_TINY still compiles these; most become
# empty stubs via #ifdef). No GUI / libvterm / xdiff / interpreters.
VIM_SRCS=(
  alloc.c arabic.c arglist.c autocmd.c beval.c blob.c blowfish.c buffer.c
  change.c charset.c cindent.c clientserver.c clipboard.c cmdexpand.c cmdhist.c
  crypt.c crypt_zip.c debugger.c dict.c diff.c digraph.c drawline.c drawscreen.c
  edit.c eval.c evalbuffer.c evalfunc.c evalvars.c evalwindow.c
  ex_cmds.c ex_cmds2.c ex_docmd.c ex_eval.c ex_getln.c
  fileio.c filepath.c findfile.c float.c fold.c fuzzy.c getchar.c gc.c
  gui_xim.c hardcopy.c hashtab.c help.c highlight.c if_cscope.c if_xcmdsrv.c
  indent.c insexpand.c json.c linematch.c list.c locale.c logfile.c
  main.c map.c mark.c match.c mbyte.c memfile.c memline.c menu.c message.c
  misc1.c misc2.c mouse.c move.c normal.c ops.c option.c optionstr.c
  os_unix.c auto/pathdef.c popupmenu.c popupwin.c profiler.c pty.c
  quickfix.c regexp.c register.c screen.c scriptfile.c search.c session.c
  sha256.c sign.c sound.c spell.c spellfile.c spellsuggest.c strings.c
  syntax.c tabpanel.c tag.c term.c terminal.c testing.c textformat.c
  textobject.c textprop.c time.c tuple.c typval.c ui.c undo.c
  usercmd.c userfunc.c version.c
  vim9class.c vim9cmds.c vim9compile.c vim9execute.c vim9expr.c
  vim9generics.c vim9instr.c vim9script.c vim9type.c
  viminfo.c window.c bufwrite.c
  myos_stubs.c
)

link_prog() {
  local arch="$1"
  shift
  local objs=("$@")
  local triple="${arch}-unknown-myos"
  local out="$ROOT/target/vim-${arch}-unknown-none"
  local prefix="$ROOT/target/newlib-${arch}"
  local lib="$prefix/${triple}/lib"
  local nclib="$ROOT/target/ncurses-${arch}/lib"
  local ld="${triple}-ld"

  "$ld" -pie --no-dynamic-linker --gc-sections -o "$out" \
    --entry=_start -z max-page-size=4096 \
    "$lib/crt0.o" "${objs[@]}" -L"$lib" -L"$nclib" \
    --start-group -lncurses -lc -lm -lgloss -lg --end-group
  "${triple}-strip" -s "$out" 2>/dev/null || strip -s "$out" 2>/dev/null || true
  echo "vim -> $out"
  if command -v llvm-size >/dev/null 2>&1; then
    llvm-size "$out" || true
  elif command -v size >/dev/null 2>&1; then
    size "$out" || true
  fi
}

build_arch() {
  local arch="$1"
  local triple="${arch}-unknown-myos"
  local prefix="$ROOT/target/newlib-${arch}"
  local inc="$prefix/${triple}/include"
  local ncinc="$ROOT/target/ncurses-${arch}/include"
  local cc="${triple}-cc"
  local objdir="$ROOT/target/vim-obj-${arch}"
  local extra=()
  local src base obj
  local objs=()
  local cppflags=(
    -DHAVE_CONFIG_H
    -DFEAT_TINY
    -D_DEFAULT_SOURCE
    -D_GNU_SOURCE
    -D_BSD_SOURCE
    -I"$WORK"
    -I"$WORK/proto"
    -I"$MYOS/include" -I"$MYOS"
    -I"$ROOT/toolchain/newlib/libgloss/myos"
    -I"$ncinc"
    -include myos_compat.h
  )

  rm -rf "$objdir"
  mkdir -p "$objdir"

  echo "==> vim ($triple)"
  for src in "${VIM_SRCS[@]}"; do
    base="$(basename "$src" .c)"
    obj="$objdir/${base}.o"
    "$cc" -ffreestanding -fPIC -O2 -std=gnu99 \
      -ffunction-sections -fdata-sections \
      -Wno-unused-parameter -Wno-unused-variable -Wno-unused-function \
      -Wno-pointer-sign -Wno-missing-field-initializers \
      -isystem "$inc" "${cppflags[@]}" \
      -c "$WORK/$src" -o "$obj"
    objs+=("$obj")
  done

  if [[ "$arch" == "aarch64" ]]; then
    "$cc" -ffreestanding -fPIC -O2 -isystem "$inc" \
      -c "$ROOT/ports/sbase/trunctfdf2.c" -o "$objdir/trunctfdf2.o"
    extra+=("$objdir/trunctfdf2.o")
  elif [[ "$arch" == "riscv64" ]]; then
    "$cc" -ffreestanding -fPIC -O2 -isystem "$inc" \
      -c "$ROOT/ports/sbase/riscv64-softfloat.c" -o "$objdir/riscv64-softfloat.o"
    extra+=("$objdir/riscv64-softfloat.o")
  fi

  link_prog "$arch" "${objs[@]}" "${extra[@]}"
}

for arch in x86_64 aarch64 riscv64; do
  build_arch "$arch"
done

echo "$(myos_vim_version_hash)" >"$MYOS_VIM_VERSION"
