# linux-compat

Userspace side of the Linux syscall compatibility layer (the kernel side is
the `modules/linux` kernel module): the `linux` launcher, the Linux-side
tests and the Alpine package fetcher. Design, scope and limits:
[`docs/linux-compat.md`](../docs/linux-compat.md).

| File | What |
|------|------|
| `launcher.c` | `linux [--root ROOT] PROGRAM [ARG...]`: run a Linux binary (native myos program, newlib); in every image |
| `build-launcher.sh` | builds the launcher for x86_64, aarch64 and riscv64 (run by `build.rs`) |
| `tests/linux-smoke.c` | boot smoke, built as a static-PIE Linux binary against musl for each arch |
| `tests/linux-dyn.c`, `tests/libsmoke*.c` | dynamically linked smoke and its shared objects |
| `build.sh` | builds the musl pieces per arch (`cargo build --features linux_compat`); fetches musl 1.2.5 (pinned sha256) and builds it with clang |
