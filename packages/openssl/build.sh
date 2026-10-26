#!/usr/bin/env bash
# Cross-build openssl(1), LibreSSL's command-line tool, for the three arches
# against the libressl port's libraries (target/libressl-<arch>), the way
# that port builds them: the source list from apps/openssl/Makefile.am
# (ports/libressl/sources.py), what configure would find on newlib +
# libgloss set by hand, LibreSSL's compat headers for what newlib lacks
# (err.h, readpassphrase.h, strtonum, freezero, pledge as a no-op).
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$HERE/../.." && pwd)"
# shellcheck source=scripts/myos-c-userspace-lib.sh
source "$ROOT/scripts/myos-c-userspace-lib.sh"
# shellcheck source=ports/libressl/versions.env
source "$ROOT/ports/libressl/versions.env"

if myos_openssl_is_current; then
  echo "openssl up to date"
  exit 0
fi

"$HERE/fetch.sh"
"$ROOT/toolchain/newlib/build.sh"
"$ROOT/ports/libressl/build.sh"
export PATH="$ROOT/target/newlib-bin:$PATH"
SRC="$ROOT/target/openssl-src/apps/openssl"
COMPAT="$ROOT/target/openssl-src/include/compat"

# What configure finds on newlib + libgloss (ports/libressl/build.sh has
# the same list), plus what the openssl program's own Makefile.am asks:
# poll and clock_gettime are there, certhash is built, this is not Windows.
HAVE=(
  ASPRINTF FTRUNCATE GETDELIM GETLINE GETOPT REALLOCARRAY STRCASECMP
  STRLCAT STRLCPY STRNDUP STRNLEN STRSEP EXPLICIT_BZERO TIMINGSAFE_BCMP
  TIMINGSAFE_MEMCMP ARC4RANDOM_BUF ARC4RANDOM_UNIFORM GETENTROPY GETPAGESIZE
  MEMMEM CLOCK_GETTIME SYSLOG ENDIAN_H MACHINE_ENDIAN_H POLL
)
HAVE_DEFS=()
HAVE_CONDS=(BUILD_CERTHASH)
for h in "${HAVE[@]}"; do
  HAVE_DEFS+=("-DHAVE_$h")
  HAVE_CONDS+=("HAVE_$h")
done

build_arch() {
  local arch="$1"
  local triple="${arch}-unknown-myos"
  local prefix="$ROOT/target/newlib-${arch}"
  local inc="$prefix/${triple}/include"
  local lib="$prefix/${triple}/lib"
  local cc="${triple}-cc"
  local ld="${triple}-ld"
  local ssl="$ROOT/target/libressl-${arch}"
  local objdir="$ROOT/target/openssl-obj-${arch}"
  local out="$ROOT/target/openssl-${arch}-unknown-none"
  local clang_res
  clang_res="$(clang -print-resource-dir)/include"
  # newlib's headers before clang's, the other way round from the library
  # build: the compat limits.h chains to the next limits.h, and clang's,
  # freestanding, chains to nothing, which left PATH_MAX undefined.
  local cflags=(
    -ffreestanding -fPIC -O2 -g0 -std=gnu99 -fno-strict-aliasing
    -ffunction-sections -fdata-sections -Wall -Wno-pointer-sign
    -Wno-unused-parameter -Wno-unused-but-set-variable -Wno-unused-variable
    -nostdinc -isystem "$inc" -isystem "$clang_res"
    -D_GNU_SOURCE
    -I"$SRC" -I"$ssl/include" -I"$COMPAT"
    -DLIBRESSL_INTERNAL -D__BEGIN_HIDDEN_DECLS= -D__END_HIDDEN_DECLS=
    -DOPENSSLDIR='"/lib/ssl"'
    -include "$HERE/myos_compat.h"
    "${HAVE_DEFS[@]}"
  )
  local extra=()
  local objs=()
  local src obj

  echo "==> openssl ($triple)"
  command -v "$cc" >/dev/null || { echo "missing compiler $cc" >&2; exit 1; }
  rm -rf "$objdir"
  mkdir -p "$objdir"
  while IFS= read -r src; do
    obj="$objdir/$(basename "${src%.c}").o"
    echo "  CC $(basename "$src") ($triple)"
    "$cc" "${cflags[@]}" -c "$src" -o "$obj"
    objs+=("$obj")
  done < <({ python3 "$ROOT/ports/libressl/sources.py" "$SRC/Makefile.am" openssl_SOURCES "${HAVE_CONDS[@]}" \
    | sed "s|^|$SRC/|"; echo "$HERE/myos_stubs.c"; })

  # The long-double helpers the sbase and lua ports link (speed and s_time
  # print timings as doubles).
  if [[ "$arch" == "aarch64" || "$arch" == "riscv64" ]]; then
    "$cc" -ffreestanding -fPIC -O2 -isystem "$inc" \
      -c "$ROOT/ports/sbase/trunctfdf2.c" -o "$objdir/trunctfdf2.o"
    extra+=("$objdir/trunctfdf2.o")
  fi

  echo "  LD openssl ($triple)"
  "$ld" -pie --no-dynamic-linker --gc-sections -o "$out" \
    --entry=_start -z max-page-size=4096 \
    "$lib/crt0.o" "${objs[@]}" "${extra[@]+"${extra[@]}"}" \
    -L"$lib" -L"$ssl/lib" \
    --start-group -lssl -lcrypto -lc -lm -lgloss -lg --end-group
  "${triple}-strip" -s "$out" 2>/dev/null || strip -s "$out" 2>/dev/null || true
  rm -rf "$objdir"
  ls -lh "$out"
}

for arch in ${MYOS_OPENSSL_ARCHES:-x86_64 aarch64 riscv64}; do
  build_arch "$arch"
done

echo "$(myos_openssl_version_hash)" >"$MYOS_OPENSSL_VERSION"
echo "openssl build ok ($LIBRESSL_VERSION)"
