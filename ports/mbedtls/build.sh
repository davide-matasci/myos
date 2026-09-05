#!/usr/bin/env bash
# Cross-build mbedtls static libs with myos newlib (clang wrappers), like sbase.
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$HERE/../.." && pwd)"
# shellcheck source=versions.env
source "$HERE/versions.env"

echo "mbedtls: HERE=$HERE ROOT=$ROOT version=$MBEDTLS_VERSION"

hash_mbedtls() {
  echo "$MBEDTLS_VERSION"
  sha256sum "$HERE/myos_mbedtls_config.h" "$HERE/build.sh" "$HERE/fetch.sh" "$HERE/versions.env" || true
  if [[ -f "$ROOT/target/cacert.pem" ]]; then
    sha256sum "$ROOT/target/cacert.pem" || true
  fi
}

STAMP="$ROOT/target/.myos-mbedtls-version"
WANT="$(hash_mbedtls | sha256sum | awk '{print $1}')"
echo "mbedtls: stamp want=$WANT"

need=0
for arch in x86_64 aarch64 riscv64; do
  if [[ ! -f "$ROOT/target/mbedtls-${arch}/lib/libmbedtls.a" ]]; then
    need=1
  fi
done
if [[ -f "$STAMP" && "$(cat "$STAMP")" == "$WANT" && "$need" -eq 0 && -f "$ROOT/target/mbedtls-ca_bundle.c" ]]; then
  echo "mbedtls libs up to date"
  exit 0
fi

echo "mbedtls: fetching..."
"$HERE/fetch.sh"
echo "mbedtls: ensuring newlib..."
"$ROOT/toolchain/newlib/build.sh"
export PATH="$ROOT/target/newlib-bin:$PATH"
echo "mbedtls: PATH has newlib-bin; cc=$(command -v x86_64-unknown-myos-cc || echo MISSING)"

SRC="$ROOT/target/mbedtls-src"
CFG="$HERE/myos_mbedtls_config.h"

python3 - <<'PY' "$ROOT/target/cacert.pem" "$ROOT/target/mbedtls-ca_bundle.c"
import pathlib, sys
pem = pathlib.Path(sys.argv[1]).read_bytes().replace(b"\r", b"")
out = pathlib.Path(sys.argv[2])
text = pem.decode("latin-1")
esc = text.replace("\\", "\\\\").replace("\"", "\\\"").replace("\n", "\\n")
step = 12000
parts = [esc[i:i+step] for i in range(0, len(esc), step)]
body = "\n".join('    "%s"' % p for p in parts)
out.write_text(
    "/* Auto-generated Mozilla CA bundle (PEM). */\n"
    "const char myos_ca_bundle_pem[] =\n" + body + ";\n"
    "const unsigned myos_ca_bundle_pem_len = sizeof(myos_ca_bundle_pem) - 1;\n"
)
print("ca bundle", len(pem), "bytes")
PY

NAMES=(
  aes asn1parse asn1write base64 bignum bignum_core cipher cipher_wrap
  constant_time ctr_drbg ecdh ecdsa ecp ecp_curves entropy error gcm md
  memory_buffer_alloc oid pem pk pk_ecc pk_wrap pkparse platform platform_util
  rsa rsa_alt_helpers sha1 sha256 sha512 ssl_ciphersuites ssl_client ssl_msg
  ssl_tls ssl_tls12_client version x509 x509_crl x509_crt
)

build_arch() {
  local arch="$1"
  local triple="${arch}-unknown-myos"
  local prefix="$ROOT/target/newlib-${arch}"
  local inc="$prefix/${triple}/include"
  local cc="${triple}-cc"
  local out="$ROOT/target/mbedtls-${arch}"
  local obj="$out/obj"
  local lib="$out/lib"
  local clang_res
  clang_res="$(clang -print-resource-dir)/include"
  local cflags=(
    -ffreestanding -fPIC -Os -g0 -nostdinc
    -isystem "$clang_res"
    -I"$HERE/include"
    -isystem "$inc"
    -I"$SRC/include"
    -I"$HERE"
    -DMBEDTLS_CONFIG_FILE='"myos_mbedtls_config.h"'
  )

  echo "mbedtls: building $arch with $cc"
  command -v "$cc" >/dev/null || { echo "missing compiler $cc"; exit 1; }
  rm -rf "$out"
  mkdir -p "$obj" "$lib"
  local name src o
  local objs=()
  for name in "${NAMES[@]}"; do
    src="$SRC/library/${name}.c"
    [[ -f "$src" ]] || continue
    o="$obj/${name}.o"
    echo "  CC $arch $name"
    "$cc" "${cflags[@]}" -c "$src" -o "$o"
    objs+=("$o")
  done
  "$cc" "${cflags[@]}" -c "$ROOT/target/mbedtls-ca_bundle.c" -o "$obj/ca_bundle.o"
  objs+=("$obj/ca_bundle.o")
  ar rcs "$lib/libmbedcrypto.a" "${objs[@]}"
  cp "$lib/libmbedcrypto.a" "$lib/libmbedtls.a"
  cp "$lib/libmbedcrypto.a" "$lib/libmbedx509.a"
  mkdir -p "$out/include"
  cp -a "$SRC/include/mbedtls" "$out/include/"
  # mbedtls 3.6 ssl.h unconditionally #includes psa/crypto.h
  cp -a "$SRC/include/psa" "$out/include/"
  cp "$CFG" "$out/include/myos_mbedtls_config.h"
  echo "mbedtls $arch: ${#objs[@]} objs"
}

build_arch x86_64
build_arch aarch64
build_arch riscv64
echo "$WANT" >"$STAMP"
echo "mbedtls build ok"
