# HTTPS on myos

## Architecture

**Wall clock:** `SYS_GETTIMEOFDAY` (29) backed by platform RTC (CMOS / PL031 / goldfish).
Required for X.509 notBefore/notAfter checks — not a workaround.

**TLS:** build-time **mbedtls** port (`ports/mbedtls/`) + reusable **`user/tls`** Rust FFI
crate. Chosen over Plan 9 `/net/tls` in netfs+netd because it is the smaller modular path;
`/net/tls` can wrap the same library later.

**DNS:** shared helper in `user/lib/src/dns.rs` used by both `dns` and `http` (no parser
duplication).

**CA bundle:** Mozilla/curl `cacert.pem` packed at mbedtls build time. Peer verification
is required (never disabled).

## Interactive use

```text
$ http https://example.com/
... Example Domain ...
https ok
$
```

Plain HTTP still works: `http 1.2.3.4 80 /` or `http http://example.com/`.
