# os-test (myos port)

Embeds the pinned [sortix/os-test](https://gitlab.com/sortix/os-test) suite into
the initramfs at `/lib/os-test` (feature `port_os_test`) and drives it with a
GNU-make harness under `overlay/`.

## Build (ports-base)

Same contract as other ports. From the repo root (host):

```sh
./ports/os-test/build.sh
```

This:

1. Fetches the pinned revision (`versions.env` → `target/os-test-src`) and
   assembles `target/os-test-embed` (upstream + `overlay/`).
2. Host-prebuilds the boot-CI smoke list into `target/os-test-prebuilt/<arch>/…`
   (all arches whose newlib sysroots exist; CI expects x86_64 + aarch64 + riscv64).
3. Writes `target/.myos-os-test-version`.

CI builds this in **ports-base** (`ci-ports.yml`), caches via `ci-registry.sh`,
and the **build** job restores artifacts into `ci-build.tar`. `build.rs` /
`initramfs.rs` **consume** those trees (no fetch/prebuild as a CI path). Local
dev: run `./ports/os-test/build.sh` once if cargo complains they are missing.

Low-level helpers (usually not needed alone):

```sh
./ports/os-test/fetch.sh
./ports/os-test/prebuild-basic-smoke.sh
```

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
#860 timed out on the full suite; #866 still hit the GHA 90m cancel on x86
under `-smp 2` heap/git + a full-tree `cp -r` before the thin copy +
fail-fast fixes; aarch64 finished the thin smoke in ~4m).

Guest staging uses a thin copy (not the whole suite):

```sh
sh /lib/os-test/misc/ci-smoke-copy.sh /tmp/o && cd /tmp/o
make SUITES=basic TESTLIST=misc/ci-basic-smoke.tests report
```

`ci-smoke-copy.sh` stages only `Makefile` + `misc/` + `basic/basic.h` + each
`.c` listed in `misc/ci-basic-smoke.tests`.

`misc/ci-basic-smoke.tests` (~22 paths) spans:

`pwd` / `grp` / `ctype` / `string` / `strings` / `stdlib` / `stdio` /
`unistd` / `signal` / `sys_stat` / `dirent` / `time` / `fcntl` / `setjmp` /
`libgen` / `arpa_inet`

It **must** include `pwd/setpwent` (hard gate). It deliberately avoids
pthread / aio / math / wchar / spawn / socket for this boot window.

CI launcher notes:

- QEMU helpers use **`-smp 4`** on x86/aarch64 (interactive + CI mini/full) so
  x86 has ≥2 APs for post-exec RR re-home / `make -j` spread; riscv stays
  **`-smp 2`** (Limine hart table panic at 4). aarch64/riscv userspace may
  still be UP.
- wait_ci overall QEMU wait is ~10m (600s), and the arrow/histrecall stage
  fail-fasts in ~20s if `histrecall_z3z` never appears (riscv64 #866 hung
  there after a good smoke + SETPWENT-OK).

CI checks:

1. Harness finished (`pass_rate=` line present for the smoke subset).
2. `basic/pwd/setpwent` success (`SETPWENT-OK`).
3. **Do not** hard-fail when `pass_rate < 80` (deferred gate).

Boot-mini skips this stage (too slow for the mini window).

## Boot CI host-prebuild (thin smoke)

The boot smoke list (`misc/ci-basic-smoke.tests`) is **host-prebuilt** into
`target/os-test-prebuilt/<arch>/basic/…` by `ports/os-test/build.sh` →
`prebuild-basic-smoke.sh` (same newlib/libgloss link as
`scripts/build-c-hello.sh`). `initramfs.rs` packs them at
`/lib/os-test/prebuilt/…`. `ci-smoke-copy.sh` stages matching ELFs;
`myos-run.sh` runs `prebuilt/$T` when present and **skips guest tcc**.

Root cause: guest `tcc … -lm` of a real os-test source under x86 TCG is
~60–100× slower than heap's tiny tcc (GHA ~90–120s/test vs ~1.5s on
aarch64). 22 guest compiles cannot fit the 600s QEMU budget; host-prebuild
keeps the smoke as real ELF exec + libc without guest compile cost.
Manual/full suite on the guest still uses tcc when prebuilts are absent.

## Full basic / nightly (follow-up)

Full `make SUITES=basic report` (~1187 tests) remains available manually on
the guest (`cp -r /lib/os-test /tmp/o` then make) and is the intended target
for a future nightly job outside the interactive boot window.
Not wired into wait_ci yet.

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
- `overlay/misc/ci-smoke-copy.sh` — thin writable staging for boot CI (copies prebuilts when present).
- `prebuild-basic-smoke.sh` — host-build smoke ELFs into `target/os-test-prebuilt/`.
