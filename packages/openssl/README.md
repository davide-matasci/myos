# openssl for myos

The `openssl` command of **LibreSSL 4.3.3**: the same release the
`libressl` port builds its libraries from (`ports/libressl/versions.env`
is the pin; `fetch.sh` extracts `apps/openssl` and the default
configuration from that tarball), cross-built against those libraries
for the three arches without LibreSSL's `configure` (`build.sh`: the
source list from `apps/openssl/Makefile.am` through the port's
`sources.py`, configure's findings for newlib + libgloss set by hand).
Keys, certificate requests and certificates, digests, base64, `s_client`
and the rest of the subcommands; the acme package does its cryptography
with it.

## On the system

```sh
get-myos openssl
run-myos openssl version
run-myos openssl ecparam -name prime256v1 -genkey -noout -out key.pem
run-myos openssl req -new -x509 -key key.pem -days 365 -subj /CN=example -out cert.pem
run-myos openssl s_client -connect example.com:443 -CAfile /lib/cacert.pem
```

`/lib/ssl/openssl.cnf` is the default configuration (`OPENSSLDIR`);
`/lib/ssl/cert.pem`, the default CA file, is not there: give `-CAfile
/lib/cacert.pem` (curl's bundle) to verify against the public CAs.

License: LibreSSL's (ISC for the new code, the OpenSSL and SSLeay
licenses for what it inherits), `licenses/LibreSSL.txt`.
