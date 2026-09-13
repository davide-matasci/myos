# os-test (myos port)

Embeds the pinned [sortix/os-test](https://gitlab.com/sortix/os-test) suite into
the initramfs at `/lib/os-test` (feature `port_os_test`) and drives it with a
GNU-make harness under `overlay/`.

## Fetch + embed

From the repo root (host):

```sh
./ports/os-test/fetch.sh
```

This clones the pinned revision into `target/os-test-src` and assembles
`target/os-test-embed` (upstream sources + `overlay/`). The initramfs builder
packs that tree at `/lib/os-test` when `port_os_test` is enabled.

## Run on the guest

`/lib/os-test` is read-only (initramfs). Copy it first:

```sh
cp -r /lib/os-test /tmp/o && cd /tmp/o
make SUITES=basic report
```

- `SUITES` selects which top-level suites to build/run (default covers basic,
  limits, io, malloc, paths, process, signal, stdio).
- `make` / `make report` compile every matched `*.c` with guest `tcc` against
  the packed newlib sysroot, run each binary, and print a summary via
  `misc/myos-report.sh`.
- Per-test outcomes live under `out/<suite>/.../*.out`:
  - empty → pass (exit 0)
  - `compile_error` → tcc could not link/build
  - `exit: N` → runtime failure

The report ends with a machine-readable line:

```text
pass_rate=NN% (P/T)
```

## CI report (full boot only)

Full-boot shell CI (`src/wait_ci.rs`, non-mini):

1. Writable copy + `make SUITES=basic report` (whole upstream basic suite).
2. Assert the harness finished (`pass_rate=` line present).
3. Still require `basic/pwd/setpwent` success (`SETPWENT-OK`).
4. **Do not** hard-fail the job when `pass_rate < 80`.

Boot-mini skips this stage (too slow for the mini window).

## Deferred 80% gate

The long-term goal is ≥80% pass on `SUITES=basic` with **real** libc/kernel
coverage — no skip-lists, XFAIL, or fake stubs that paint the suite green.
Until that bar is honest and stable, CI only **reports** `pass_rate=` and
gates on harness completion + the setpwent regression, not on the percentage.

Full basic is ~1187 tests and may need ~90 minutes under TCG; GitHub Actions
`boot` jobs and QEMU wait timeouts may need to be raised in lockstep when
enabling that gate.

## Overlay layout

- `overlay/Makefile` — GNU make harness (no `$(shell …)` forks).
- `overlay/misc/myos-run.sh` — compile+run one test into `out/…`.
- `overlay/misc/myos-report.sh` — pass/fail/compile_error + `pass_rate=` summary.
