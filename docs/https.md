# HTTPS on myos

## Architecture

**Wall clock:** `SYS_GETTIMEOFDAY` (29) backed by platform RTC (CMOS / PL031 / goldfish).
Required for X.509 notBefore/notAfter checks — not a workaround.

**TLS:** build-time **mbedtls** port (`ports/mbedtls/`) + reusable **`user/tls`** Rust FFI
crate. Chosen over Plan 9 `/net/tls` in netfs+netd because it is the smaller modular path;
`/net/tls` can wrap the same library later.

**TLS heap:** ~2 MiB mbedtls arena fed to `mbedtls_memory_buffer_alloc_init`.
On aarch64/riscv64 it is allocated at runtime via page-aligned `brk`
(`myos_tls_alloc_heap`) so it is not ELF BSS. A 2 MiB BSS sat immediately
before the user stack (~610-page `http` image); an MPI over-read walked
image→stack→pre-mapped heap and faulted just past `heap_limit`
(`stval≈0x40466000` on riscv64 CI). x86_64 keeps the BSS arena: the brk path
hit mbedtls `ALLOC_FAILED` (-32512) on bios/uefi even with 1 GiB QEMU.

**Aspace / freelist (riscv stability):** on-demand `sys_brk` now SFENCE.VMA
after mapping (stale non-present TLB entries caused mid-heap faults once the
arena moved to brk). In-place exec always reclaims the heap window, and
`sys_munmap` returns frames to the freelist (and only accepts anonymous mmap
regions) so exec-heavy smoke cannot drain UEFI RAM into “random” user faults.

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
