#!/usr/bin/env bash
# Cross-build freestanding mbedtls static libs for myos arches (clang).
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$HERE/../.." && pwd)"
# shellcheck source=versions.env
source "$HERE/versions.env"

hash_mbedtls() {
  {
    echo "$MBEDTLS_VERSION"
    sha256sum "$HERE/mbedtls_config.h" "$HERE/build.sh" "$HERE/fetch.sh" "$HERE/versions.env" 2>/dev/null
    [[ -f "$ROOT/target/cacert.pem" ]] && sha256sum "$ROOT/target/cacert.pem"
  } | sha256sum | awk '{print $1}'
}

STAMP="$ROOT/target/.myos-mbedtls-version"
WANT="$(hash_mbedtls)"
need_build=0
for arch in x86_64 aarch64 riscv64; do
  if [[ ! -f "$ROOT/target/mbedtls-${arch}/lib/libmbedtls.a" ]]; then
    need_build=1
  fi
done
if [[ -f "$STAMP" && "$(cat "$STAMP")" == "$WANT" && "$need_build" -eq 0 && -f "$ROOT/target/mbedtls-ca_bundle.c" ]]; then
  echo "mbedtls libs up to date"
  exit 0
fi

"$HERE/fetch.sh"
SRC="$ROOT/target/mbedtls-src"
CFG="$HERE/mbedtls_config.h"

python3 - <<'PY' "$ROOT/target/cacert.pem" "$ROOT/target/mbedtls-ca_bundle.c"
import pathlib, sys
pem = pathlib.Path(sys.argv[1]).read_bytes().replace(b"\r", b"")
out = pathlib.Path(sys.argv[2])
esc = pem.replace(b"\\", b"\\\\").replace(b"\"", b"\\\"").replace(b"\n", b"\\n")
step = 12000
parts = [esc[i:i+step] for i in range(0, len(esc), step)]
body = "\n".join('    "%s"' % p.decode("ascii") for p in parts)
out.write_text(
    "/* Auto-generated Mozilla CA bundle (PEM). */\n"
    "const char myos_ca_bundle_pem[] =\n" + body + ";\n"
    "const unsigned myos_ca_bundle_pem_len = sizeof(myos_ca_bundle_pem) - 1;\n"
)
print("ca bundle", len(pem), "bytes")
PY

# Explicit client-oriented sources (mbedtls 3.6 library/).
SRCS=(
  aes aesni aria asn1parse asn1write base64 bignum bignum_core camellia ccm chacha20
  chachapoly cipher cipher_wrap cmac constant_time ctr_drbg des dhm ecdh ecdsa ecjpake
  ecp ecp_curves entropy error gcm hkdf hmac_drbg lmots lms md md5
  memory_buffer_alloc nist_kw oid padlock pem pk pk_ecc pk_wrap pkcs12 pkcs5
  pkparse platform platform_util poly1305 psa_crypto psa_crypto_aead psa_crypto_cipher
  psa_crypto_client psa_crypto_driver_wrappers_no_static psa_crypto_ecp
  psa_crypto_hash psa_crypto_mac psa_crypto_pake psa_crypto_rsa psa_crypto_se
  psa_crypto_slot_management psa_crypto_storage psa_util ripemd160 rsa rsa_alt_helpers
  sha1 sha256 sha3 sha512 ssl_cache ssl_ciphersuites ssl_client ssl_cookie
  ssl_debug_helpers_generated ssl_msg ssl_ticket ssl_tls ssl_tls12_client
  ssl_tls12_server ssl_tls13_client ssl_tls13_generic ssl_tls13_keys ssl_tls13_server
  threading timing version version_features x509 x509_create x509_crl x509_crt x509_csr
  x509write_crt x509write_csr
)

# Prefer a tighter set — only compile files that exist.
build_arch() {
  local arch="$1"
  local triple="${arch}-unknown-none"
  local out="$ROOT/target/mbedtls-${arch}"
  local obj="$out/obj"
  local lib="$out/lib"
  local cflags=(
    --target="$triple"
    -ffreestanding -fno-builtin -fPIC -Os -g0
    -I"$SRC/include"
    -I"$HERE"
    -DMBEDTLS_CONFIG_FILE='"mbedtls_config.h"'
  )
  if [[ "$arch" == "riscv64" ]]; then
    cflags+=(-march=rv64imac -mabi=lp64)
  fi

  rm -rf "$out"
  mkdir -p "$obj" "$lib"
  local name src o
  local objs=()
  for name in "${SRCS[@]}"; do
    src="$SRC/library/${name}.c"
    [[ -f "$src" ]] || continue
    o="$obj/${name}.o"
    if ! clang "${cflags[@]}" -c "$src" -o "$o" 2>"$obj/${name}.err"; then
      # Skip sources rejected by config (common with psa_* when PSA disabled).
      rm -f "$o"
      continue
    fi
    objs+=("$o")
  done
  clang "${cflags[@]}" -c "$ROOT/target/mbedtls-ca_bundle.c" -o "$obj/ca_bundle.o"
  objs+=("$obj/ca_bundle.o")
  llvm-ar rcs "$lib/libmbedcrypto.a" "${objs[@]}"
  cp "$lib/libmbedcrypto.a" "$lib/libmbedtls.a"
  cp "$lib/libmbedcrypto.a" "$lib/libmbedx509.a"
  mkdir -p "$out/include"
  cp -a "$SRC/include/mbedtls" "$out/include/"
  cp -a "$SRC/include/psa" "$out/include/" 2>/dev/null || true
  cp "$CFG" "$out/include/mbedtls_config.h"
  echo "mbedtls $arch: ${#objs[@]} objs -> $lib/libmbedtls.a"
}

build_arch x86_64
build_arch aarch64
build_arch riscv64
echo "$WANT" >"$STAMP"
echo "mbedtls build ok"
