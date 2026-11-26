#!/bin/sh
# Cross-build LibreSSL's static libcrypto, libssl and libtls for the three
# arches, against each C library, without its configure (which compiles and
# runs test programs a cross build cannot run): the source lists come from
# its automake files (sources.py), and what configure would probe for is set
# below, per libc. newlib's build goes to target/libressl-<arch>/ (lib/,
# include/openssl, include/tls.h), where curl, lynx, nginx and user/tls
# build against it; mlibc's to target/libressl-mlibc-<arch>/, for the mlibc
# ports (python): a libc's errno, stdio and struct layouts are its own, so a
# library serves the programs of the libc it was built against only.
#
# MYOS_LIBRESSL_ARCHES and MYOS_LIBRESSL_LIBCS name what to build, all of it
# by default (scripts/myos-c-userspace-lib.sh). On myos
# (scripts/self-host.sh) that is the arch it runs on, against newlib: what
# user/tls links.
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
ROOT="$(cd "$HERE/../.." && pwd)"
# shellcheck source=versions.env
. "$HERE/versions.env"
# shellcheck source=scripts/myos-c-userspace-lib.sh
. "$ROOT/scripts/myos-c-userspace-lib.sh"

if myos_libressl_is_current; then
  echo "libressl libs up to date"
  exit 0
fi

"$HERE/fetch.sh"
for libc in $MYOS_LIBRESSL_LIBCS; do
  "$ROOT/toolchain/$libc/build.sh"
done
export PATH="$ROOT/target/newlib-bin:$PATH"
SRC="$ROOT/target/libressl-src"

# What configure finds on newlib + libgloss (m4/check-libc.m4): these
# functions and headers it has, so their compat copies stay out of the
# build; the others (strtonum, freezero, recallocarray, getprogname,
# syslog_r, err.h, ...) come from crypto/compat and include/compat.
HAVE_NEWLIB="
  ASPRINTF FTRUNCATE GETDELIM GETLINE GETOPT REALLOCARRAY STRCASECMP
  STRLCAT STRLCPY STRNDUP STRNLEN STRSEP EXPLICIT_BZERO TIMINGSAFE_BCMP
  TIMINGSAFE_MEMCMP ARC4RANDOM_BUF ARC4RANDOM_UNIFORM GETENTROPY GETPAGESIZE
  MEMMEM CLOCK_GETTIME SYSLOG ENDIAN_H MACHINE_ENDIAN_H
"
# And on mlibc: the same less the timingsafe compares (compat's then) and
# <machine/endian.h>; the arc4random family is the shared C's
# (toolchain/libmyos/random.c), as newlib has its own.
HAVE_MLIBC="
  ASPRINTF FTRUNCATE GETDELIM GETLINE GETOPT REALLOCARRAY STRCASECMP
  STRLCAT STRLCPY STRNDUP STRNLEN STRSEP EXPLICIT_BZERO ARC4RANDOM_BUF
  ARC4RANDOM_UNIFORM GETENTROPY GETPAGESIZE MEMMEM CLOCK_GETTIME SYSLOG ENDIAN_H
"
HAVE_DEFS=""
HAVE_CONDS=""
# have NAME...: the HAVE_ defines and automake conditions of a libc.
have() {
  HAVE_DEFS=""
  HAVE_CONDS=""
  for _have_h in "$@"; do
    HAVE_DEFS="$HAVE_DEFS -DHAVE_$_have_h"
    HAVE_CONDS="$HAVE_CONDS HAVE_$_have_h"
  done
}

# The sources of VAR in DIR's Makefile.am, for the host conditions of ARCH.
sources() (
  dir="$1" var="$2" host="$3"
  # shellcheck disable=SC2086
  sh "$HERE/sources.sh" "$SRC/$dir/Makefile.am" "$var" $HAVE_CONDS "$host" \
    | sed "s|^|$SRC/$dir/|"
)

# compile ARCH-CC OBJDIR FLAGS... -- SRC: one object, its errors kept. The
# text of a script, which xargs runs in a shell of its own: its arguments
# are the compiler and its flags, then the source and the object, which it
# brings to the front first.
# shellcheck disable=SC2016
COMPILE_ONE='n=$(($# - 2))
while [ "$n" -gt 0 ]; do
  set -- "$@" "$1"
  shift
  n=$((n - 1))
done
src="$1" obj="$2"
shift 2
if ! "$@" -c "$src" -o "$obj" 2>"$obj.err"; then
  echo "FAIL $src" >&2
  head -20 "$obj.err" >&2
  exit 1
fi'

# compile_all OBJDIR OBJS-FILE CC FLAGS... with the sources on stdin: in
# parallel, the object paths appended to OBJS-FILE.
compile_all() (
  objdir="$1" list="$2"
  shift 2
  pairs="$list.pairs"
  : >"$pairs"
  while IFS= read -r src; do
    obj="$objdir/$(echo "${src#"$SRC/"}" | tr '/' '_')"
    obj="${obj%.c}.o"
    printf '%s\0%s\0' "$src" "$obj" >>"$pairs"
    echo "$obj" >>"$list"
  done
  # xargs appends each pair after the compiler and its flags.
  xargs -0 -n 2 -P "$(myos_nproc)" sh -c "$COMPILE_ONE" _ "$@" <"$pairs"
)

# build_arch ARCH LIBC
build_arch() (
  arch="$1" libc="$2"
  triple="${arch}-unknown-myos"
  prefix="$ROOT/target/newlib-${arch}"
  inc="$prefix/${triple}/include"
  clang_res="$(clang -print-resource-dir)/include"
  # The compiler's flags are the positional parameters: the libc's first.
  if [ "$libc" = mlibc ]; then
    cc=clang
    out="$ROOT/target/libressl-mlibc-${arch}"
    # shellcheck disable=SC2046
    set -- $(myos_mlibc_cflags "$arch")
    # shellcheck disable=SC2086
    have $HAVE_MLIBC
  else
    cc="${triple}-cc"
    out="$ROOT/target/libressl-${arch}"
    set -- -nostdinc -isystem "$clang_res" -isystem "$inc"
    # shellcheck disable=SC2086
    have $HAVE_NEWLIB
    # ui_openssl.c reads passwords through termios (the ncurses/vim pattern).
    cp "$ROOT/toolchain/newlib/libgloss/myos/termios.h" "$inc/termios.h"
  fi
  obj="$out/obj"
  case "$arch" in
    x86_64) march=amd64 host=HOST_X86_64 ;;
    aarch64) march=aarch64 host=HOST_AARCH64 ;;
    riscv64) march=riscv64 host=HOST_RISCV64 ;;
  esac

  # Makefile.am.common and the libraries' own flags, less the assembly
  # (OPENSSL_NO_ASM: the C versions everywhere, as on an unknown host). A
  # library's hidden/ headers come before include/ (its symbol aliases).
  # shellcheck disable=SC2086
  set -- \
    -ffreestanding -fPIC -O2 -g0 -std=gnu99 -fno-strict-aliasing \
    -ffunction-sections -fdata-sections -Wall -Wno-pointer-sign \
    -Wno-unused-parameter -Wno-unused-but-set-variable -Wno-unused-variable \
    "$@" \
    -D_GNU_SOURCE \
    -I"$SRC/include" -I"$SRC/include/compat" \
    -I"$SRC/crypto/arch/$march" -I"$SRC/crypto/bn/arch/$march" \
    -DLIBRESSL_INTERNAL -DOPENSSL_NO_HW_PADLOCK -DOPENSSL_NO_ASM \
    -D__BEGIN_HIDDEN_DECLS= -D__END_HIDDEN_DECLS= \
    -DOPENSSLDIR='"/lib/ssl"' \
    $HAVE_DEFS
  crypto_inc=""
  for d in aes asn1 bio bn bytestring conf curve25519 dh dsa ec ecdh ecdsa err \
    evp hmac lhash mlkem modes ocsp pkcs12 rsa sha stack x509; do
    crypto_inc="$crypto_inc -I$SRC/crypto/$d"
  done

  echo "==> libressl ($triple, $libc)"
  command -v "$cc" >/dev/null || { echo "missing compiler $cc" >&2; exit 1; }
  rm -rf "$out"
  mkdir -p "$obj/crypto" "$obj/ssl" "$obj/tls" "$out/lib" "$out/include"

  # libcrypto, with the compat functions newlib lacks (libcompat.la).
  : >"$obj/crypto.objs"
  # shellcheck disable=SC2086
  { sources crypto libcrypto_la_SOURCES "$host"; sources crypto libcompat_la_SOURCES "$host"; } \
    | compile_all "$obj/crypto" "$obj/crypto.objs" "$cc" -I"$SRC/crypto/hidden" \
      -DLIBRESSL_CRYPTO_INTERNAL -I"$SRC/crypto" $crypto_inc "$@"
  # libssl, with the bytestring objects it carries (libbs.la).
  : >"$obj/ssl.objs"
  { sources ssl libssl_la_SOURCES "$host"; sources ssl libbs_la_SOURCES "$host"; } \
    | compile_all "$obj/ssl" "$obj/ssl.objs" "$cc" -I"$SRC/ssl/hidden" \
      -I"$SRC/crypto" -I"$SRC/crypto/bio" "$@"
  # libtls: the CA bundle curl's port ships as /lib/cacert.pem is its default.
  : >"$obj/tls.objs"
  sources tls libtls_la_SOURCES "$host" \
    | compile_all "$obj/tls" "$obj/tls.objs" "$cc" "$@" \
      -DTLS_DEFAULT_CA_FILE='"/lib/cacert.pem"'

  # Apple BSD ar symbol tables are unreadable by rust-lld for ELF targets;
  # prefer llvm-ar (brew llvm / nightly rust sysroot) when available.
  ar_bin="$(command -v llvm-ar 2>/dev/null || echo ar)"
  for lib in crypto ssl tls; do
    xargs "$ar_bin" rcs "$out/lib/lib$lib.a" <"$obj/$lib.objs"
  done
  cp -R "$SRC/include/openssl" "$out/include/"
  rm -f "$out/include/openssl/Makefile.am" "$out/include/openssl/Makefile.in"
  cp "$SRC/include/tls.h" "$out/include/"
  rm -rf "$obj"
  echo "libressl $arch $libc: $(ls "$out/lib")"
)

for arch in $MYOS_LIBRESSL_ARCHES; do
  for libc in $MYOS_LIBRESSL_LIBCS; do
    build_arch "$arch" "$libc"
  done
done

myos_self_host || echo "$(myos_libressl_version_hash)" >"$MYOS_LIBRESSL_VERSION"
echo "libressl build ok ($LIBRESSL_VERSION)"
