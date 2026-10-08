# os-test (myos port)

Embeds the pinned [sortix/os-test](https://gitlab.com/sortix/os-test) suite into
the initramfs at `/lib/os-test` (feature `port_os_test`) and drives it with a
GNU-make harness under `overlay/`.

## Build (CI ports job)

Same contract as other ports. From the repo root (host):

```sh
./packages/os-test/build.sh
```

This:

1. Fetches the pinned revision (`versions.env` → `target/os-test-src`) and
   assembles `target/os-test-embed` (upstream + `overlay/`).
2. Host-prebuilds the boot-CI smoke list into `target/os-test-prebuilt/<arch>/…`
   (all arches whose newlib sysroots exist; CI expects x86_64 + aarch64 + riscv64).
3. Writes `target/.myos-os-test-version`.

CI builds this in the **ports** job (`ci-ports.yml`), caches via `ci-registry.sh`,
and the **build** job restores artifacts into `ci-build.tar`. `build.rs` /
`initramfs.rs` **consume** those trees (no fetch/prebuild as a CI path). Local
dev: run `./packages/os-test/build.sh` once if cargo complains they are missing.

Low-level helpers (usually not needed alone):

```sh
./packages/os-test/fetch.sh
./packages/os-test/prebuild-basic-smoke.sh
```

## Run on the guest

`/lib/os-test` is read-only (initramfs). Copy it first:

```sh
cp -r /lib/os-test /tmp/o && cd /tmp/o
make SUITES=basic report
```

- `SUITES` selects which top-level suites to build/run (default covers basic,
  limits, io, malloc, paths, process, signal, stdio, udp, myos).
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
- Per-test outcomes live under `out/<suite>/.../*.out`, written as upstream's
  `misc/run.sh` writes them: the test's output, then `exit: N` when the
  output is empty or N is 2 or more (`compile_error` when tcc could not
  build it).
- `misc/myos-report.sh` grades them as upstream's `misc/html.c` does: a test
  with expectations (`<suite>.expect/<test>.*`, read from `/lib/os-test`
  when the staged copy has none) passes when its outcome equals one of them
  (`*.unknown.*` included: outcomes upstream has seen but not judged); a
  test without any passes on `exit: 0`. So
  `open: ENOTDIR` passes where POSIX wants that error, and a `printf` test
  passes only when it printed the right text.

The report ends with a machine-readable line:

```text
pass_rate=NN% (P/T)
```

## Boot test curated set (full list only)

The full boot test (`packages/os-test/test.sh`, `docs/testing.md`) runs a
**thin curated set**: the basic smoke list **plus** ~100 tests spanning
non-basic suites (`limits`, `io`, `malloc`, `paths`, `process`, `signal`,
`stdio`, `udp`) **plus** `misc/ci-expansion.tests` (155: POSIX core, more
non-basic, signal handlers, and the myos `chroot`/FIFO suite) **plus**
`misc/ci-expansion-2.tests` (502: every other test that builds and passes on
all three arches). 833 tests, all host-prebuilt. See `SUITES.md` for the full
suite inventory, the selection rationale and the deferred tests.

Guest staging uses a thin copy (not the whole suite):

```sh
sh /lib/os-test/misc/ci-smoke-copy.sh /tmp/o && cd /tmp/o
make TESTLIST=misc/ci-boot.tests report
```

`ci-boot.tests` includes `ci-basic-smoke.tests` + `ci-nonbasic-100.tests` +
`ci-expansion.tests` + `ci-expansion-2.tests`.
`ci-smoke-copy.sh` stages `Makefile` + `misc/` + suite headers + each listed
`.c` (suite-prefixed paths for non-basic; basic-relative for the smoke list).

Basic smoke **must** include `pwd/setpwent` (hard gate). Non-basic picks
prefer high-value syscall/libc coverage; missing kernel/libc support is
implemented for real (no skip/XFAIL/fake stubs).

Launcher notes:

- `cat /proc/meminfo` shows the kernel frame counters (`FramesLive`, …);
  live frames stay flat across the curated run, so the test keeps 4096 MiB
  (UEFI 4608).
- QEMU uses **`-smp 4`** on x86/aarch64 so x86 has ≥2 APs for post-exec RR
  re-home / `make -j` spread; riscv stays **`-smp 2`** (Limine hart table
  panic at 4).
- The report's progress (one line per test) streams to the console, which
  keeps the host's stall watchdog quiet; the full run has a 50-minute budget.

The test passes only when **every curated test passed** (`pass_rate=100%
(T/T)`); otherwise it prints the report's failure list. A test that cannot
pass yet belongs in `SUITES.md` (deferred), not in the curated lists. A
second test checks `basic/pwd/setpwent` (its `.out` must be `exit: 0`).

The mini list skips os-test (too slow for the mini window).

## Boot CI host-prebuild (thin smoke)

The boot curated list (`misc/ci-boot.tests`) is **host-prebuilt** into
`target/os-test-prebuilt/<arch>/basic/…` by `packages/os-test/build.sh` →
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
Not wired into the boot tests yet.

## Pass-rate gate

The curated full-boot set (`misc/ci-boot.tests`) is gated at **100%**: every
listed test must pass on every arch. The long-term goal is ≥80% pass on the
full `SUITES=basic` with **real** libc/kernel coverage — no skip-lists, XFAIL,
or fake stubs that paint the suite green; tests are added to the curated
lists only once they pass honestly.

## Overlay layout

- `overlay/Makefile` — GNU make harness (no `$(shell …)` forks; supports
  `TESTS` / `AREAS` / `TESTLIST`).
- `overlay/misc/myos-run.sh` — compile+run one test into `out/…` (prints
  `os-test: <path>` progress before each compile).
- `overlay/misc/myos-report.sh` — pass/fail/compile_error + `pass_rate=` summary.
- `overlay/misc/ci-basic-smoke.tests` — boot CI smoke list (`TESTS +=` paths).
- `overlay/misc/ci-expansion.tests` — POSIX core + non-basic + myos expansion.
- `overlay/misc/ci-expansion-2.tests` — the rest of the suite that passes on every arch.
- `overlay/myos/` — myos-specific tests (`chroot/`, `fifo/`) + `myos.h` helpers.
- `overlay/misc/ci-smoke-copy.sh` — thin writable staging for boot CI (copies prebuilts when present).
- `prebuild-basic-smoke.sh` — host-build smoke ELFs into `target/os-test-prebuilt/`.
