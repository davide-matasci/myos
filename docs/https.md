# HTTPS on myos

myos programs speak TLS 1.2 and 1.3 through LibreSSL. The `http` command
fetches HTTPS URLs, and any Rust program can do the same through the
`user/tls` crate.

## Interactive use

```text
$ http https://example.com/
... Example Domain ...
https ok
$
```

Plain HTTP still works: `http 1.2.3.4 80 /` or `http http://example.com/`.

## How it works

### TLS

- **LibreSSL** is a build-time port (`ports/libressl/`). It builds
  libcrypto, libssl and libtls, static, for the three arches.
- **`user/tls`** is a reusable Rust FFI crate over libtls, LibreSSL's
  small client API: `tls_client`, `tls_connect_cbs` over the `/net` data
  fd, `tls_handshake`, `tls_read`/`tls_write`.

This was chosen over a Plan 9 `/net/tls` in netfs and netd because
it is the smaller, more modular path. A `/net/tls` can wrap the same
library later.

### Certificates

The CA bundle is Mozilla's, as curl distributes it (`cacert.pem`). The
curl port fetches it and the image ships it as `/lib/cacert.pem`, where
`user/tls` reads it. Peer verification is always required, never
disabled.

### Wall clock

X.509 checks a certificate's notBefore and notAfter dates, so TLS needs
the real time. This is a requirement, not a workaround.

- `SYS_GETTIMEOFDAY` (33) reads the wall clock. The platform RTC backs it
  (CMOS, PL031 or goldfish).
- `SYS_SETTIMEOFDAY` (65) moves it until the next boot. `date
  MMDDhhmmYYYY` and `clock_settime` use it. The RTC is not written.

### Heap

LibreSSL mallocs through newlib, while the program's Rust allocator
(`user/lib`, `alloc.rs`) bumps the same program break. Both re-read the
break before growing it (libgloss `_sbrk`, `alloc.rs`). So each only ever
extends the break from where it is, and neither moves it back over the
other's memory.

### Entropy

libgloss's `getentropy()` reads the kernel's `getrandom` (`/dev/urandom`).
LibreSSL and newlib's `arc4random` draw from it.

### DNS

`dns` and `http` share one resolver helper, `user/lib/src/dns.rs`, so
the parser is not duplicated.
