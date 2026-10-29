#!/usr/bin/env bash
# Makes the test certificate authority and the launcher's server
# certificate (ca.pem, server.pem, server.key here; committed, like
# dropbear's test key): the launcher's HTTPS server (src/test_www.rs)
# presents server.pem, the server's certificate and the intermediate CA
# that signed it (as a site's chain is: a client must link the two), and
# the guest's tests trust ca.pem alone
# (/lib/myos-tests/test-ca.pem), never the image's bundle. The guest
# reaches the host as 10.0.2.2. Valid for a century: nothing renews them.
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
openssl ecparam -name prime256v1 -genkey -noout -out "$tmp/ca.key"
openssl req -x509 -new -key "$tmp/ca.key" -sha256 -days 36500 \
  -subj "/CN=myos test CA" -out ca.pem \
  -addext "basicConstraints=critical,CA:TRUE" -addext "keyUsage=critical,keyCertSign,cRLSign"
openssl ecparam -name prime256v1 -genkey -noout -out "$tmp/int.key"
openssl req -new -key "$tmp/int.key" -subj "/CN=myos test intermediate CA" -out "$tmp/int.csr"
printf '%s\n' "basicConstraints=critical,CA:TRUE,pathlen:0" "keyUsage=critical,keyCertSign,cRLSign" > "$tmp/int.ext"
openssl x509 -req -in "$tmp/int.csr" -CA ca.pem -CAkey "$tmp/ca.key" -CAcreateserial -CAserial "$tmp/ca.srl" \
  -sha256 -days 36500 -extfile "$tmp/int.ext" -out "$tmp/int.pem"
openssl ecparam -name prime256v1 -genkey -noout -out "$tmp/server.ec"
openssl pkcs8 -topk8 -nocrypt -in "$tmp/server.ec" -out server.key
openssl req -new -key server.key -subj "/CN=10.0.2.2" -out "$tmp/server.csr"
printf '%s\n' "basicConstraints=CA:FALSE" "keyUsage=critical,digitalSignature" \
  "extendedKeyUsage=serverAuth" "subjectAltName=IP:10.0.2.2,IP:127.0.0.1,DNS:localhost" > "$tmp/ext"
openssl x509 -req -in "$tmp/server.csr" -CA "$tmp/int.pem" -CAkey "$tmp/int.key" -CAcreateserial \
  -CAserial "$tmp/int.srl" -sha256 -days 36500 -extfile "$tmp/ext" -out "$tmp/server.pem"
cat "$tmp/server.pem" "$tmp/int.pem" > server.pem
