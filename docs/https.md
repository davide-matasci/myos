# HTTPS on myos

## Architecture

**Wall clock:** `SYS_GETTIMEOFDAY` (33) backed by platform RTC (CMOS / PL031 / goldfish).
Required for X.509 notBefore/notAfter checks — not a workaround. `SYS_SETTIMEOFDAY`
(65; `date MMDDhhmmYYYY`, `clock_settime`) moves it until the next boot; the RTC is not
written.

**TLS:** build-time **LibreSSL** port (`ports/libressl/`: libcrypto, libssl and
libtls, static, for the three arches) + reusable **`user/tls`** Rust FFI crate over
**libtls**, LibreSSL's small client API (`tls_client`, `tls_connect_cbs` over the
`/net` data fd, `tls_handshake`, `tls_read`/`tls_write`). Chosen over Plan 9
`/net/tls` in netfs+netd because it is the smaller modular path; `/net/tls` can wrap
the same library later. TLS 1.2 and 1.3.

**Heap:** LibreSSL mallocs through newlib, while the program's Rust allocator
(`user/lib`, `alloc.rs`) bumps the same program break. Both re-read the break
before growing it (libgloss `_sbrk`, `alloc.rs`), so each only ever extends it
from where it is and neither moves it back over the other's memory.

**Entropy:** libgloss `getentropy()` over the kernel's `getrandom` (`/dev/urandom`),
what LibreSSL and newlib's `arc4random` draw from.

**DNS:** shared helper in `user/lib/src/dns.rs` used by both `dns` and `http` (no parser
duplication).

**CA bundle:** Mozilla/curl `cacert.pem`, which the curl port fetches and the image
ships as `/lib/cacert.pem`; `user/tls` reads it there. Peer verification is required
(never disabled).

## Interactive use

```text
$ http https://example.com/
... Example Domain ...
https ok
$
```

Plain HTTP still works: `http 1.2.3.4 80 /` or `http http://example.com/`.
