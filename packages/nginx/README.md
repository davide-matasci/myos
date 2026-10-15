# nginx for myos

Upstream **nginx 1.30.6** (stable; fetched at build, not vendored),
cross-built for the three arches without its `configure`, which compiles
and runs test programs a cross build cannot run. What `configure` would
write is hand-written instead (the vim, lynx and curl approach):

| File | Role |
|------|------|
| `ngx_auto_config.h`, `ngx_auto_headers.h` | `configure`'s results for newlib + libgloss: the generic POSIX path (no `NGX_LINUX`), the modules, the paths |
| `build.sh` | the sources of `auto/sources` for that path, `objs/ngx_modules.c` as `auto/modules` writes it, the link |
| `myos_compat.h`, `myos_stubs.c` | what newlib lacks that has no honest implementation: rlimits (`getrlimit` reports the fd table, 256), `setpriority`'s declaration, `timezone` |
| `nginx.c.myos.patch` | creates `/tmp/nginx` at startup (myos has no `/var`) |
| `ngx_process.c.myos.patch` | no `FIOASYNC`/`F_SETOWN` on the workers' channels (no `SIGIO` on fds); no `si_pid` in the signal log line (issue #451) |

Modules: the default set (static, index, autoindex, proxy, fastcgi, uwsgi,
scgi, memcached, rewrite, map, geo, limit_*, auth_basic with nginx's own
password schemes, gzip, ssi, ...) and ssl, events through `poll`, regular
expressions with `packages/pcre2`, gzip with `ports/zlib`, TLS with
`ports/mbedtls`.

## HTTPS: the OpenSSL API over mbedtls

nginx's SSL code is written against OpenSSL; myos has one TLS library,
mbedtls (curl's, lynx's). `ssl/` is the part of the OpenSSL 1.1.1 API
nginx uses, implemented over it: `myos_openssl.h` (every `<openssl/*.h>`
includes it), `myos_crypto.c` (errors, stacks, BIOs, digests, keys, the
random generator, from the kernel's `getrandom`), `myos_x509.c`
(certificates, names, CRLs, PEM) and `myos_ssl.c` (contexts, connections,
sessions). What works:

- TLS 1.2 servers and clients (`proxy_pass https://`), ECDHE and RSA key
  exchange with AES-GCM and AES-CBC, RSA and EC certificates (one of each
  per server, as OpenSSL), chains, keys in PEM (PKCS#8, RSA, EC);
- SNI: the server of the name the client sends, its certificates;
  `proxy_ssl_server_name`;
- client certificates (`ssl_verify_client`, `ssl_client_certificate`,
  `ssl_trusted_certificate`, `ssl_crl`, `ssl_verify_depth`), the upstream's
  (`proxy_ssl_verify`, `proxy_ssl_trusted_certificate`, its name);
- `ssl_session_cache shared:...` (resumption through nginx's cache),
  `ssl_session_timeout`;
- `ssl_ciphers` with OpenSSL's names of the suites above (a keyword such
  as `HIGH` means mbedtls's own list, strong suites only), `ssl_ecdh_curve`
  (prime256v1, secp384r1), `ssl_prefer_server_ciphers`;
- the variables (`$ssl_protocol`, `$ssl_cipher`, `$ssl_server_name`,
  `$ssl_client_verify`, `$ssl_client_s_dn`, the fingerprint, serial and
  dates, the certificate in PEM, ...); `$ssl_ciphers` is the suite
  chosen (what the client offered is not kept).

What does not, the way an OpenSSL build without it says so to nginx:
TLS 1.3 (mbedtls's needs its PSA crypto, which the myos build leaves
out), HTTP/2 and ALPN (issue #453), OCSP stapling, `ssl_session_tickets` keys, early
data, `ssl_dhparam` (ECDHE only), `ssl_conf_command`, encrypted keys
(`ssl_password_file`), engines, variables in `ssl_certificate`.

## On the system

```sh
get-myos nginx
run-myos nginx            # serves /lib/nginx/html on port 80
run-myos nginx -t         # checks /lib/nginx/nginx.conf
run-myos nginx -c FILE    # another configuration
run-myos nginx -s quit    # stops it
```

The app's files are at the paths nginx was built for: the binary
`/bin/custom/nginx`, the configuration `/lib/nginx/nginx.conf` with
upstream's `mime.types`, the default pages `/lib/nginx/html`. A relative
path in a configuration is relative to `/lib/nginx`. nginx writes its
pid, its logs and its temporary files under `/tmp/nginx`.

A master forks the workers (`worker_processes`), which share the
listening sockets, a shared-memory accept mutex (`MAP_SHARED |
MAP_ANON`) and the channels the master talks to them over (`socketpair`,
fds passed with `SCM_RIGHTS`). `listen` takes any address, 127.0.0.0/8 or
the host's own (`docs/sockets-curl.md`). The `user` directive defaults to
root: myos has no superuser to give up, and `setuid` changes nothing.

## Test

`test.sh` (the full test list, after the install): the default
configuration checks; a server of the test's own on 127.0.0.1 answers a
page, a 404, a directory listing, a regex location, a rewrite and a
gzip-compressed response, logs them, and stops on `nginx -s quit`; then
HTTPS: a page over TLS 1.2, the server of the name sent (SNI, an RSA
certificate), a verified client certificate, plain HTTP to the HTTPS port
answered 400, and nginx proxying to itself over HTTPS with the upstream's
certificate and name verified. The certificates and keys are the test's
own (`test/mkcerts.py`, self-signed, packed with the tests).
