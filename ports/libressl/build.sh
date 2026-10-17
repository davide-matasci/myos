#!/usr/bin/env bash
# Cross-build LibreSSL's static libcrypto, libssl and libtls for the three
# arches with myos newlib, without its configure (which compiles and runs
# test programs a cross build cannot run): the source lists come from its
# automake files (sources.py), and what configure would probe for is set
# below, for newlib + libgloss. The result goes to target/libressl-<arch>/
# (lib/, include/openssl, include/tls.h), where curl, lynx, nginx and
# user/tls build against it.
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$HERE/../.." && pwd)"
# shellcheck source=versions.env
source "$HERE/versions.env"
# shellcheck source=scripts/myos-c-userspace-lib.sh
source "$ROOT/scripts/myos-c-userspace-lib.sh"

if myos_libressl_is_current; then
  echo "libressl libs up to date"
  exit 0
fi

"$HERE/fetch.sh"
"$ROOT/toolchain/newlib/build.sh"
export PATH="$ROOT/target/newlib-bin:$PATH"
SRC="$ROOT/target/libressl-src"

# What configure finds on newlib + libgloss (m4/check-libc.m4): these
# functions and headers it has, so their compat copies stay out of the
# build; the others (strtonum, freezero, recallocarray, getprogname,
# syslog_r, err.h, ...) come from crypto/compat and include/compat.
HAVE=(
  ASPRINTF FTRUNCATE GETDELIM GETLINE GETOPT REALLOCARRAY STRCASECMP
  STRLCAT STRLCPY STRNDUP STRNLEN STRSEP EXPLICIT_BZERO TIMINGSAFE_BCMP
  TIMINGSAFE_MEMCMP ARC4RANDOM_BUF ARC4RANDOM_UNIFORM GETENTROPY GETPAGESIZE
  MEMMEM CLOCK_GETTIME SYSLOG ENDIAN_H MACHINE_ENDIAN_H
)
HAVE_DEFS=()
HAVE_CONDS=()
for h in "${HAVE[@]}"; do
  HAVE_DEFS+=("-DHAVE_$h")
  HAVE_CONDS+=("HAVE_$h")
done

# The sources of VAR in DIR's Makefile.am, for the host conditions of ARCH.
sources() {
  local dir="$1" var="$2" host="$3"
  python3 "$HERE/sources.py" "$SRC/$dir/Makefile.am" "$var" "${HAVE_CONDS[@]}" "$host" \
    | sed "s|^|$SRC/$dir/|"
}

# compile ARCH-CC OBJDIR FLAGS... -- SRC: one object, its errors kept.
compile_one() {
  local src="$1" obj="$2"
  shift 2
  if ! "$@" -c "$src" -o "$obj" 2>"$obj.err"; then
    echo "FAIL $src" >&2
    head -20 "$obj.err" >&2
    return 1
  fi
}
export -f compile_one

# compile_all OBJDIR OBJS-FILE CC FLAGS... with the sources on stdin: in
# parallel, the object paths appended to OBJS-FILE.
compile_all() {
  local objdir="$1" list="$2"
  shift 2
  local src obj
  local pairs=()
  while IFS= read -r src; do
    obj="$objdir/$(echo "${src#"$SRC/"}" | tr '/' '_')"
    obj="${obj%.c}.o"
    pairs+=("$src" "$obj")
    echo "$obj" >>"$list"
  done
  # xargs appends each pair after the compiler and its flags.
  printf '%s\0' "${pairs[@]}" \
    | xargs -0 -n 2 -P "$(nproc)" bash -c 'n=$#; compile_one "${@:n-1:1}" "${@:n}" "${@:1:n-2}"' _ "$@"
}

build_arch() {
  local arch="$1"
  local triple="${arch}-unknown-myos"
  local prefix="$ROOT/target/newlib-${arch}"
  local inc="$prefix/${triple}/include"
  local cc="${triple}-cc"
  local out="$ROOT/target/libressl-${arch}"
  local obj="$out/obj"
  local march host
  case "$arch" in
    x86_64) march=amd64 host=HOST_X86_64 ;;
    aarch64) march=aarch64 host=HOST_AARCH64 ;;
    riscv64) march=riscv64 host=HOST_RISCV64 ;;
  esac
  local clang_res
  clang_res="$(clang -print-resource-dir)/include"
  # ui_openssl.c reads passwords through termios (the ncurses/vim pattern).
  cp "$ROOT/toolchain/newlib/libgloss/myos/termios.h" "$inc/termios.h"

  # Makefile.am.common and the libraries' own flags, less the assembly
  # (OPENSSL_NO_ASM: the C versions everywhere, as on an unknown host). A
  # library's hidden/ headers come before include/ (its symbol aliases).
  local cflags=(
    -ffreestanding -fPIC -O2 -g0 -std=gnu99 -fno-strict-aliasing
    -ffunction-sections -fdata-sections -Wall -Wno-pointer-sign
    -Wno-unused-parameter -Wno-unused-but-set-variable -Wno-unused-variable
    -nostdinc -isystem "$clang_res" -isystem "$inc"
    -D_GNU_SOURCE
    -I"$SRC/include" -I"$SRC/include/compat"
    -I"$SRC/crypto/arch/$march" -I"$SRC/crypto/bn/arch/$march"
    -DLIBRESSL_INTERNAL -DOPENSSL_NO_HW_PADLOCK -DOPENSSL_NO_ASM
    -D__BEGIN_HIDDEN_DECLS= -D__END_HIDDEN_DECLS=
    -DOPENSSLDIR='"/lib/ssl"'
    "${HAVE_DEFS[@]}"
  )
  local crypto_inc=()
  local d
  for d in aes asn1 bio bn bytestring conf curve25519 dh dsa ec ecdh ecdsa err \
    evp hmac lhash mlkem modes ocsp pkcs12 rsa sha stack x509; do
    crypto_inc+=(-I"$SRC/crypto/$d")
  done

  echo "==> libressl ($triple)"
  command -v "$cc" >/dev/null || { echo "missing compiler $cc" >&2; exit 1; }
  rm -rf "$out"
  mkdir -p "$obj/crypto" "$obj/ssl" "$obj/tls" "$out/lib" "$out/include"

  # libcrypto, with the compat functions newlib lacks (libcompat.la).
  : >"$obj/crypto.objs"
  { sources crypto libcrypto_la_SOURCES "$host"; sources crypto libcompat_la_SOURCES "$host"; } \
    | compile_all "$obj/crypto" "$obj/crypto.objs" "$cc" -I"$SRC/crypto/hidden" \
      -DLIBRESSL_CRYPTO_INTERNAL -I"$SRC/crypto" "${crypto_inc[@]}" "${cflags[@]}"
  # libssl, with the bytestring objects it carries (libbs.la).
  : >"$obj/ssl.objs"
  { sources ssl libssl_la_SOURCES "$host"; sources ssl libbs_la_SOURCES "$host"; } \
    | compile_all "$obj/ssl" "$obj/ssl.objs" "$cc" -I"$SRC/ssl/hidden" \
      -I"$SRC/crypto" -I"$SRC/crypto/bio" "${cflags[@]}"
  # libtls: the CA bundle curl's port ships as /lib/cacert.pem is its default.
  : >"$obj/tls.objs"
  sources tls libtls_la_SOURCES "$host" \
    | compile_all "$obj/tls" "$obj/tls.objs" "$cc" "${cflags[@]}" \
      -DTLS_DEFAULT_CA_FILE='"/lib/cacert.pem"'

  # Apple BSD ar symbol tables are unreadable by rust-lld for ELF targets;
  # prefer llvm-ar (brew llvm / nightly rust sysroot) when available.
  local ar_bin
  ar_bin="$(command -v llvm-ar 2>/dev/null || echo ar)"
  local lib
  for lib in crypto ssl tls; do
    xargs "$ar_bin" rcs "$out/lib/lib$lib.a" <"$obj/$lib.objs"
  done
  cp -a "$SRC/include/openssl" "$out/include/"
  rm -f "$out/include/openssl/Makefile.am" "$out/include/openssl/Makefile.in"
  cp "$SRC/include/tls.h" "$out/include/"
  rm -rf "$obj"
  echo "libressl $arch: $(ls "$out/lib")"
}

for arch in ${MYOS_LIBRESSL_ARCHES:-x86_64 aarch64 riscv64}; do
  build_arch "$arch"
done

echo "$(myos_libressl_version_hash)" >"$MYOS_LIBRESSL_VERSION"
echo "libressl build ok ($LIBRESSL_VERSION)"
