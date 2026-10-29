# HTTPS on myos

myos programs speak TLS 1.2 and 1.3 through LibreSSL. The `http` command
fetches HTTPS URLs, and any Rust program can do the same through the
`user/tls` crate.

```text
$ http https://example.com/
... Example Domain ...
https ok
```

Plain HTTP still works: `http 1.2.3.4 80 /` or `http http://example.com/`.

## How it works

- **LibreSSL** is a port (`ports/libressl/`): static libcrypto, libssl
  and libtls for the three arches.
- **`user/tls`** is a Rust FFI crate over libtls, LibreSSL's small client
  API, running TLS over a `/net` data fd. This is smaller and more
  modular than a Plan 9 `/net/tls` in netfs and netd; such a file could
  wrap the same library later.
- **Certificates.** The CA bundle is Mozilla's, as curl distributes it.
  The curl port fetches it and the image ships it as `/lib/cacert.pem`.
  Peer verification is always on.
- **Wall clock.** Certificate dates need the real time.
  `SYS_GETTIMEOFDAY` reads the platform RTC (CMOS, PL031 or goldfish).
  `SYS_SETTIMEOFDAY` (`date MMDDhhmmYYYY`, `clock_settime`) moves it until
  the next boot, without writing the RTC.
- **Heap.** LibreSSL mallocs through newlib while the Rust allocator
  (`user/lib`, `alloc.rs`) grows the same program break. Both re-read the
  break before growing it (libgloss `_sbrk`), so neither overwrites the
  other.
- **Entropy.** libgloss's `getentropy()` reads the kernel's `getrandom`,
  which LibreSSL and newlib's `arc4random` draw from.
- **DNS.** `dns` and `http` share one resolver, `user/lib/src/dns.rs`.
