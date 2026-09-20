#!/usr/bin/env bash
# Build dropbear 2026.94 (sshd + dbclient) for myos: x86_64 aarch64 riscv64.
# No configure: hand-written config.h/localoptions.h + direct compiles
# (same pattern as ports/curl). Static bundled libtomcrypt/libtommath.
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$HERE/../.." && pwd)"
# shellcheck source=versions.env
source "$HERE/versions.env"
# shellcheck source=scripts/myos-c-userspace-lib.sh
source "$ROOT/scripts/myos-c-userspace-lib.sh"

STAMP="$MYOS_DROPBEAR_VERSION"

pack_dropbear_aliases() {
  # CI packs coreutils-* globs for C ports; keep the alias in sync (curl pattern).
  local arch src alias
  for arch in x86_64 aarch64 riscv64; do
    src="$ROOT/target/dropbear-${arch}-unknown-none"
    alias="$ROOT/target/coreutils-dropbear-${arch}-unknown-none"
    if [[ -f "$src" ]]; then
      cp "$src" "$alias"
    elif [[ -f "$alias" ]]; then
      cp "$alias" "$src"
    fi
  done
}

if myos_dropbear_is_current; then
  echo "dropbear ELFs up to date"
  pack_dropbear_aliases
  exit 0
fi

"$HERE/fetch.sh"
"$HERE/prepare.sh"
"$ROOT/toolchain/newlib/build.sh"
export PATH="$ROOT/target/newlib-bin:$PATH"
myos_ensure_llvm_bin

WORK="$ROOT/target/dropbear-myos-build"
LTC="$WORK/libtomcrypt"
LTM="$WORK/libtommath"
OUT_COMMON=("-ldbm" -Dbmain -Dbmain)

rm -f "$ROOT/target/.myos-dropbear-version"

# ---- libtomcrypt.a per arch (plain C; exclude test/primitives dirs) ----
build_ltc() {
  local arch="$1" triple="$2"
  local inc="$ROOT/target/newlib-${arch}/${triple}/include"
  local lib="$ROOT/target/newlib-${arch}/${triple}/lib"
  local out="$ROOT/target/dropbear-ltc-${arch}.a"
  local cc="${triple}-cc"
  local objs=()
  local f base
  while IFS= read -r f; do
    base="ltc_${arch}_$(echo "$f" | sed 's|^src/||; s|/|_|g; s|\.c$||')"
    local o="$ROOT/target/${base}.o"
    if [[ ! -f "$o" ]]; then
      # sober128tab.c is a bare table file (no includes) — feed it the typedef header.
      local extra=()
      [[ "$f" == *sober128tab.c ]] && extra=("-include" "tomcrypt_cfg.h")
      "$cc" -ffreestanding -fPIC -O2 -DLTC_SOURCE -I"$LTC/src/headers" -I"$LTM" -I"$WORK/src" \
        -include "$HERE/myos_compat.h" -isystem "$inc" \
        "${extra[@]}" -c "$LTC/$f" -o "$o"
    fi
    objs+=("$o")
  done < <(cd "$LTC" && find src -name '*.c' ! -path 'src/primitives/*' | sort)
  rm -f "$out"
  ar crs "$out" "${objs[@]}"
  echo "$out"
}

# ---- libtommath.a per arch ----
build_ltm() {
  local arch="$1" triple="$2"
  local inc="$ROOT/target/newlib-${arch}/${triple}/include"
  local out="$ROOT/target/dropbear-ltm-${arch}.a"
  local cc="${triple}-cc"
  local objs=()
  local f base
  while IFS= read -r f; do
    base="ltm_${arch}_$(echo "$f" | sed 's|^|L|; s|/|_|g; s|\.c$||')"
    local o="$ROOT/target/${base}.o"
    if [[ ! -f "$o" ]]; then
      "$cc" -ffreestanding -fPIC -O2 -I"$LTM" -I"$WORK/src" -isystem "$inc" -c "$LTM/$f" -o "$o"
    fi
    objs+=("$o")
  done < <(cd "$LTM" && find . -maxdepth 1 -name '*.c' | sort)
  rm -f "$out"
  ar crs "$out" "${objs[@]}"
  echo "$out"
}

for arch in x86_64 aarch64 riscv64; do
  triple="${arch}-unknown-myos"
  none_triple="${arch}-unknown-none"
  prefix="$ROOT/target/newlib-${arch}"
  inc="$prefix/${triple}/include"
  lib="$prefix/${triple}/lib"
  cc="${triple}-cc"
  echo "==> dropbear ($arch)"

  # -nostdinc: never leak host glibc headers into the guest build
  # (newlib + clang resource + port stubs only).
  clang_res="$(clang -print-resource-dir)/include"
  dbflags=(-ffreestanding -fPIC -O2 -g -nostdinc -DDEBUG_TRACE=4
    -Wno-incompatible-function-pointer-types
    -isystem "$clang_res" -isystem "$inc" -I"$HERE/include" -I"$LTM")

  LTC_A="$(build_ltc "$arch" "$triple")"
  LTM_A="$(build_ltm "$arch" "$triple")"

  # float helpers (same as tcp-listen smoke): aarch64 needs trunctfdf2,
  # riscv64 needs the soft-float shim (LLVM refolds integer tricks into
  # float compares; the shim is the only surviving implementation).
  extra_objs=()
  if [[ "$arch" == "aarch64" ]]; then
    extra_objs+=("$ROOT/target/tcp-listen-smoke-aarch64-trunctfdf2.o")
    if [[ ! -f "$ROOT/target/tcp-listen-smoke-aarch64-trunctfdf2.o" ]]; then
      "$cc" -ffreestanding -fPIC -O2 -isystem "$inc" \
        -c "$ROOT/ports/sbase/trunctfdf2.c" -o "$ROOT/target/tcp-listen-smoke-aarch64-trunctfdf2.o"
    fi
  elif [[ "$arch" == "riscv64" ]]; then
    extra_objs+=("$ROOT/target/tcp-listen-smoke-riscv64-softfloat.o")
    if [[ ! -f "$ROOT/target/tcp-listen-smoke-riscv64-softfloat.o" ]]; then
      "$cc" -ffreestanding -fPIC -O2 -isystem "$inc" \
        -c "$ROOT/ports/sbase/riscv64-softfloat.c" -o "$ROOT/target/tcp-listen-smoke-riscv64-softfloat.o"
    fi
  fi

  # dropbear objects
  objs=()
  while IFS= read -r f; do
    base="db_${arch}_$(basename "$f" .c)"
    local_o="$ROOT/target/${base}.o"
    if [[ ! -f "$local_o" ]]; then
      "$cc" "${dbflags[@]}" \
        -I"$WORK/src" -I"$LTC/src/headers" \
        -DHAVE_CONFIG_H -D_GNU_SOURCE \
        -include "$HERE/myos_compat.h" \
        -c "$WORK/$f" -o "$local_o"
    fi
    objs+=("$local_o")
  done < <(cd "$WORK" && ls src/*.c | sort)
  # port shims
  shim_o="$ROOT/target/db_${arch}_myos_shims.o"
  "$cc" "${dbflags[@]}" -I"$WORK/src" -include "$HERE/myos_compat.h" -c "$HERE/myos_shims.c" -o "$shim_o"
  builtins_o="$ROOT/target/dropbear-builtins-${arch}.o"
  "$cc" "${dbflags[@]}" -c "$HERE/myos_builtins.c" -o "$builtins_o"

  # link sshd (dropbear) and dbclient via dropbearmulti-style split ELFs
  # SVR = common + clisvr + svr; CLI = common + clisvr + cli
  mapfile -t SVR < <(cd "$WORK" && ls src/dbutil.c src/buffer.c src/dbhelpers.c src/dss.c src/bignum.c \
    src/signkey.c src/rsa.c src/dbrandom.c src/queue.c src/atomicio.c src/compat.c \
    src/fake-rfc2553.c src/ltc_prng.c src/ecc.c src/ecdsa.c src/sk-ecdsa.c src/crypto_desc.c \
    src/curve25519.c src/ed25519.c src/sk-ed25519.c src/dbmalloc.c src/dbctype.c \
    src/gensignkey.c src/gendss.c src/genrsa.c src/gened25519.c \
    src/common-session.c src/packet.c src/common-algo.c src/common-kex.c \
    src/common-channel.c src/common-chansession.c src/termcodes.c src/loginrec.c \
    src/tcp-accept.c src/listener.c src/process-packet.c src/dh_groups.c \
    src/common-runopts.c src/circbuffer.c src/list.c src/netio.c src/chachapoly.c src/gcm.c \
    src/kex-x25519.c src/kex-dh.c src/kex-ecdh.c src/kex-pqhybrid.c \
    src/sntrup761.c src/mlkem768.c \
    src/svr-kex.c src/svr-auth.c src/sshpty.c src/svr-authpubkey.c \
    src/svr-authpubkeyoptions.c src/svr-session.c src/svr-service.c \
    src/svr-chansession.c src/svr-runopts.c src/svr-main.c src/svr-tcpfwd.c \
    src/svr-streamfwd.c src/svr-forward.c 2>/dev/null | sort -u)
  mapfile -t CLI < <(cd "$WORK" && ls src/dbutil.c src/buffer.c src/dbhelpers.c src/dss.c src/bignum.c \
    src/signkey.c src/rsa.c src/dbrandom.c src/queue.c src/atomicio.c src/compat.c \
    src/fake-rfc2553.c src/ltc_prng.c src/ecc.c src/ecdsa.c src/sk-ecdsa.c src/crypto_desc.c \
    src/curve25519.c src/ed25519.c src/sk-ed25519.c src/dbmalloc.c src/dbctype.c \
    src/gensignkey.c src/gendss.c src/genrsa.c src/gened25519.c \
    src/common-session.c src/packet.c src/common-algo.c src/common-kex.c \
    src/common-channel.c src/common-chansession.c src/termcodes.c src/loginrec.c \
    src/tcp-accept.c src/listener.c src/process-packet.c src/dh_groups.c \
    src/common-runopts.c src/circbuffer.c src/list.c src/netio.c src/chachapoly.c src/gcm.c \
    src/kex-x25519.c src/kex-dh.c src/kex-ecdh.c src/kex-pqhybrid.c \
    src/sntrup761.c src/mlkem768.c \
    src/cli-main.c src/cli-auth.c src/cli-authpasswd.c src/cli-kex.c \
    src/cli-session.c src/cli-runopts.c src/cli-chansession.c \
    src/cli-authpubkey.c src/cli-tcpfwd.c src/cli-channel.c src/cli-readconf.c \
    2>/dev/null | sort -u)

  o_of() {
    echo "$ROOT/target/db_${arch}_$(basename "$1" .c).o"
  }

  svr_objs=()
  for f in "${SVR[@]}"; do svr_objs+=("$(o_of "$f")"); done
  cli_objs=()
  for f in "${CLI[@]}"; do cli_objs+=("$(o_of "$f")"); done

  ld.lld -pie --no-dynamic-linker -o "$ROOT/target/dropbear-${none_triple}" \
    --entry=_start -z max-page-size=4096 \
    "$lib/crt0.o" "${svr_objs[@]}" "$shim_o" "$builtins_o" "$LTC_A" "$LTM_A" "${extra_objs[@]+"${extra_objs[@]}"}" \
    -L"$lib" --start-group -lc -lgloss -lg --end-group || { echo "sshd link failed ($arch)"; exit 1; }

  ld.lld -pie --no-dynamic-linker -o "$ROOT/target/dbclient-${none_triple}" \
    --entry=_start -z max-page-size=4096 \
    "$lib/crt0.o" "${cli_objs[@]}" "$shim_o" "$builtins_o" "$LTC_A" "$LTM_A" "${extra_objs[@]+"${extra_objs[@]}"}" \
    -L"$lib" --start-group -lc -lgloss -lg --end-group || { echo "dbclient link failed ($arch)"; exit 1; }

  # dropbearkey: host-key generation in-guest (pre-generate instead of -R delay)
  key_objs=()
  for f in $(cd "$WORK" && ls src/dropbearkey.c); do
    key_objs+=("$(o_of "$f")")
  done
  ld.lld -pie --no-dynamic-linker -o "$ROOT/target/dropbearkey-${none_triple}" \
    --entry=_start -z max-page-size=4096 \
    "$lib/crt0.o" "${key_objs[@]}" "${svr_objs[@]:0:0}" \
    $(for f in src/dbutil.c src/buffer.c src/dbhelpers.c src/dss.c src/bignum.c src/signkey.c src/rsa.c src/dbrandom.c src/queue.c src/atomicio.c src/compat.c src/fake-rfc2553.c src/ltc_prng.c src/ecc.c src/ecdsa.c src/sk-ecdsa.c src/crypto_desc.c src/curve25519.c src/ed25519.c src/sk-ed25519.c src/dbmalloc.c src/dbctype.c src/gensignkey.c src/gendss.c src/genrsa.c src/gened25519.c; do echo "$(o_of "$f")"; done) \
    "$shim_o" "$builtins_o" "$LTC_A" "$LTM_A" "${extra_objs[@]+"${extra_objs[@]}"}" \
    -L"$lib" --start-group -lc -lgloss -lg --end-group || { echo "dropbearkey link failed ($arch)"; exit 1; }

  echo "==> dropbear ($arch) linked: dropbear-${none_triple} dbclient-${none_triple} dropbearkey-${none_triple}"
done

pack_dropbear_aliases

hash="$(myos_dropbear_version_hash)"
printf '%s\n' "$hash" > "$ROOT/target/.myos-dropbear-version"
echo "dropbear -> target/dropbear-* + target/dbclient-* (stamp .myos-dropbear-version)"
