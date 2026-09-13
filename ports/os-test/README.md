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
- Optional narrowers (prefer these for interactive / CI smokes):
  - `TESTS="pwd/setpwent ctype/isalpha …"` — explicit paths relative to each
    suite (no `.c` suffix).
  - `TESTLIST=misc/ci-basic-smoke.tests` — same list from a checked-in
    makefile fragment (`TESTS += path` one per line; `include`, no `$(shell)`).
  - `AREAS=pwd,grp,ctype` — all `*.c` under those subdirs of each suite.
- When `TESTS` / `AREAS` / `TESTLIST` are unset, `make` builds every matched
  `*.c` under `SUITES` (full suite — fine manually, too heavy for the 90m
  boot CI window).
- `make` / `make report` compile with guest `tcc` against the packed newlib
  sysroot, run each binary, and print a summary via `misc/myos-report.sh`.
- Per-test outcomes live under `out/<suite>/.../*.out`:
  - empty → pass (exit 0)
  - `compile_error` → tcc could not link/build
  - `exit: N` → runtime failure

The report ends with a machine-readable line:

```text
pass_rate=NN% (P/T)
```

## Boot CI smoke subset (full boot only)

Full-boot shell CI (`src/wait_ci.rs`, non-mini) runs a **thin curated subset**
of upstream `basic/` — a little of everything, not all ~1187 tests (CI run
#860 timed out when the boot job tried the full suite in the 90m window):

```sh
cp -r /lib/os-test /tmp/o && cd /tmp/o
make SUITES=basic TESTLIST=misc/ci-basic-smoke.tests report
```

`misc/ci-basic-smoke.tests` (~22 paths) spans:

`pwd` / `grp` / `ctype` / `string` / `strings` / `stdlib` / `stdio` /
`unistd` / `signal` / `sys_stat` / `dirent` / `time` / `fcntl` / `setjmp` /
`libgen` / `arpa_inet`

It **must** include `pwd/setpwent` (hard gate). It deliberately avoids
pthread / aio / math / wchar / spawn / socket for this boot window.

CI checks:

1. Harness finished (`pass_rate=` line present for the smoke subset).
2. `basic/pwd/setpwent` success (`SETPWENT-OK`).
3. **Do not** hard-fail when `pass_rate < 80` (deferred gate).

Boot-mini skips this stage (too slow for the mini window).

## Full basic / prebuild / nightly (follow-up)

Full `make SUITES=basic report` (~1187 tests) remains available manually on
the guest and is the intended target for a future prebuild or nightly job
outside the interactive 90m boot window. Not wired into wait_ci yet.

## Deferred 80% gate

The long-term goal is ≥80% pass on full `SUITES=basic` with **real**
libc/kernel coverage — no skip-lists, XFAIL, or fake stubs that paint the
suite green. Until that bar is honest and stable, CI only **reports**
`pass_rate=` (for the smoke subset today) and gates on harness completion +
the setpwent regression, not on the percentage.

## Overlay layout

- `overlay/Makefile` — GNU make harness (no `$(shell …)` forks; supports
  `TESTS` / `AREAS` / `TESTLIST`).
- `overlay/misc/myos-run.sh` — compile+run one test into `out/…` (prints
  `os-test: <path>` progress before each compile).
- `overlay/misc/myos-report.sh` — pass/fail/compile_error + `pass_rate=` summary.
- `overlay/misc/ci-basic-smoke.tests` — boot CI smoke list (`TESTS +=` paths).
