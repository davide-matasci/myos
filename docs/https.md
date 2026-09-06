# HTTPS on myos

## Architecture

**Wall clock:** `SYS_GETTIMEOFDAY` (29) backed by platform RTC (CMOS / PL031 / goldfish).
Required for X.509 notBefore/notAfter checks — not a workaround.

**TLS:** build-time **mbedtls** port (`ports/mbedtls/`) + reusable **`user/tls`** Rust FFI
crate. Chosen over Plan 9 `/net/tls` in netfs+netd because it is the smaller modular path;
`/net/tls` can wrap the same library later.

**TLS heap:** the ~2 MiB mbedtls arena is allocated at runtime via page-aligned `brk`
(`myos_tls_alloc_heap`), not ELF BSS. A BSS arena sat in the image immediately before
the user stack; an MPI over-read could walk image→stack→heap and fault at `heap_limit`
after corrupting on-stack TLS state (intermittent riscv64 CI page faults).

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
