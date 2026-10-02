# linux-compat (optional)

Userspace side of the **optional** Linux syscall compatibility layer: the
`linux` launcher and the Linux-side tests. Only built with
`cargo build --features linux_compat` (x86_64, aarch64, riscv64); a default build ignores this
directory. Design, scope and limits: [`docs/linux-compat.md`](../docs/linux-compat.md).

| File | What |
|------|------|
| `launcher.c` | `linux PROGRAM [ARG...]`: run a Linux binary (native myos program, newlib) |
| `tests/linux-smoke.c` | boot smoke, built as a static-PIE Linux binary against musl for each arch |
| `build.sh` | builds both per arch; fetches musl 1.2.5 (pinned sha256) and builds it with clang |
