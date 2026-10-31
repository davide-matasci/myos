# Boot tests

The real test of myos is booting it. The launcher boots an image headless
in QEMU, logs in and runs a list of tests **in the guest**. The host only
watches the serial console for the results. Read this page when you add a
test, run the tests locally or wonder why CI booted what it booted.

## Running the tests

```sh
cargo run -- test                    # x86_64 BIOS, every test (needs the network)
cargo run -- aarch64 test            # also uefi, riscv64
cargo run -- test --skip net,pkg,heavy,install,alpine-disk   # the quick list: no network, a few minutes (`test-mini`)
cargo run -- test --tags smoke       # the smoke set alone
cargo run -- test --tags ssh,https   # these two: a test's name is a tag of its own
scripts/local-ci.sh [bios|uefi|aarch64|riscv64] [mini|full|test args...]   # the same, with the OOM/TCG settings for a loaded host
```

A boot without `test` is a normal boot to the login prompt, which is what
the ISO is for.

QEMU emulates the guest's CPU (TCG) unless told otherwise. On a host with
`/dev/kvm`, `MYOS_X86_ACCEL=kvm` runs the x86 boots on the host's CPU
(`-accel kvm`). The CPU model is `max` either way: under TCG everything
QEMU emulates, under KVM the host's CPU, both with XSAVE and AVX, as
real machines have them (`arch/x86/fpu.rs`). The light list then takes about 400 s instead of 670 to 950. CI does
this for its x86 boots. aarch64 and riscv64 stay under TCG on an x86 host.

## Selecting tests

Every test carries **tags**: what it needs, what it covers, and its own
name. `test` takes the tests with any of the `--tags` (all tests without
`--tags`), minus those with any of the `--skip` tags. So `--tags dns` runs
that one test, and `--skip heap` leaves that one out. `test-mini` and
`test-full` are the two lists of old: `test --skip
net,pkg,heavy,install,alpine-disk` and `test`.

| tag | the tests that |
|---|---|
| `net` | need the host's network: the package mirror and the test web server the launcher serves, GitHub, the Alpine mirror |
| `pkg` | need the packages of this build, installed as apps or from the mirror itself. Every package's own test has `pkg` and its package's name |
| `heavy` | take minutes under TCG: the curated os-test list, the Alpine apk installs, the heavy smoke with the apps |
| `alpine-disk` | mount the Alpine roots the host prepares: rustc, Python, an X client (`linux-compat/alpine-disks.sh`, built once into `target/alpine-<arch>.img`, attached as `/dev/nvme2n1/data`) |
| `install` | change the boot disk: `get-myos --upgrade` and `--install` on the scratch disk (`docs/install.md`) |
| `smoke` | are the quick basics: the shell, exec, the basic programs, the filesystems on the scratch disk, `heap mini`; about two minutes |
| a module's name (`netfs`, `console`, `linux`, `ext2`, `fat`, `usb`, `blk`, `virtio_net`, ...) or an area's (`sec`, `tty`, `events`, `mm`, `sched`, `proc`, `fs`, `exec`, `x11`, `pci`) | cover it: what a change there must run |
| a port's name (`oksh`, `c-smokes`, `curl`, `os-test`, ...) | come from that port's `test.sh`. The runner sets it on every test of the file. |
| `mlibc` | run a C smoke built against mlibc instead of newlib (`user/c-mlibc/test.sh`, the `mlibc_` tests). Each is also tagged with its newlib test's name (`docs/mlibc.md`). |
| `alpine`, `python`, `rust`, `x11`, `ostest` | the long tests, by what they build, for a run of one of them alone |

### Budgets and hung tests

- A quick run (`heavy` and `net` skipped) has a budget of 8 minutes, 9
  with the Linux layer, and a stall limit of 3 minutes. riscv64, the
  slowest under TCG, gets half as much again, as in CI.
- Any other run has a budget of 50 minutes and a stall limit of 10.
- `--budget MINUTES` sets the budget.
- A stall (nothing on the console within the limit) ends the run with
  every CPU's registers from the QEMU monitor (`info registers -a`);
  `addr2line` on the kernel shows where the guest spins or halts.
- A test still running after 2 minutes (5 when `heavy` is not skipped)
  gets a `HUNG <name>` line on the console. After it, once, come every
  thread's `/proc/<pid>/task/<tid>/status` line and every process's
  command line. The status line shows the thread's state and, in `wait`,
  the key it is blocked on (`docs/proc.md`; `kernel/src/task/sched.rs`
  lists the key ranges). So a hung test shows what waits on what before
  the host gives up. It was added after CI #2295's lynx test stalled with
  every CPU idle and the console showed nothing more.

## Writing a test

Every test belongs to the port of what it tests and ships with it: the
shell's tests are oksh's, the ext2 ones `mkfs.ext2`'s, listen/accept and
the tty the C smokes' (`user/c/test.sh`). Where to put a new one:

- A port's or a program's behaviour: a function and a `t` line in the
  `test.sh` next to its `port.env` (`PORT_TEST=test.sh`, `docs/ports.md`).
  Programs the tests need (smokes) go in `user/c` or as a `user` crate,
  with their test.
- The kernel's own behaviour (a limit, a module without a user-side
  program): `user/tests/kernel.sh`, tagged with the module or area.
- A POSIX conformance case: the curated os-test lists
  (`packages/os-test/overlay/misc/*.tests`), which must pass in full.

```sh
# packages/make/test.sh
make_version() {
	run-myos make --version | grep -q "GNU Make" && [ ! -e /bin/custom/make ]
}
t make make_version
```

- `t -t TAG,TAG name function` tags the test. Tag what it needs (`net`,
  `pkg`, `heavy`, `install`) and what it covers (the module's or area's
  name). The needs decide which runs it can be in; the coverage decides
  which changes must run it. The port's own name is on every test of its
  file already, and `pkg` on every test of a package.
- A `pkg` test names in its tags the packages it runs (`alpine_jq` names
  `get-alpine`, the X clients `tinyx`), and runs them with `run-myos`
  (`docs/packages.md`). A test that names none gets none.
- `$OUT` is the output directory. `contains NEEDLE FILE` greps.
- fd 3 is the console (`echo "HOST ..." >&3`). A long test can stream its
  progress there, as os-test does.
- Keep the output of a passing test to itself. Failures show the last 40
  lines.

Test on every arch the change could affect; arch-specific code needs all
three. A quick run per arch is usually enough locally, and CI does the
rest.

### A peer on the host

A test that needs a peer outside the guest (a client connecting in) gets
one from the port's own `host.sh` (`PORT_HOST=host.sh`). The launcher
knows no test:

1. The guest side prints `HOST <port> <args>` to fd 3.
2. The launcher runs `bash <port dir>/host.sh <args>` on the host, in the
   background.
3. The guest side waits, with a bound, for the effect: a file the session
   touched, a reply on its socket.

Its exit status is logged, but the guest test decides. Several `HOST`
requests run in parallel. A test boot's QEMU monitor is a unix socket the
scripts find in `$MYOS_QEMU_MONITOR`: `user/c/host.sh sendkey KEYS...`
types on the guest's keyboard through it, `user/c/host.sh mouse` moves
and clicks its mice.

### The test web server

The tests that fetch over HTTP(S) use the launcher's test web server
(`src/test_www.rs`), not a site on the internet, whose answers vary from
run to run. It serves `/hello.txt` on the guest's `10.0.2.2:8766` (HTTP)
and `10.0.2.2:8767` (HTTPS, with a certificate of the test CA in
`user/tests/tls`, made by its `make.sh` and committed, signed through an
intermediate CA the server sends along, as a site's is). The tests trust
that CA alone (`/lib/myos-tests/test-ca.pem`), never the image's bundle.

What needs the real internet uses GitHub: `dns` and the Linux layer's
lookups resolve `github.com`, and the kernel's HTTPS client (`https`)
fetches `https://github.com/robots.txt`. In a sandbox behind a
TLS-intercepting proxy, append its CA to `target/cacert.pem` for local
runs only, and restore it afterwards.

## In the guest

### The tests module

The tests are not in the image. The ISO and the release's boot files
carry none of them. They are the **tests module**, a second zstd cpio
archive beside the initramfs (`build_boot_archives` in
`src/initramfs.rs`):

- Only the build's disks carry it (`target/*.img`), as `boot/tests` on the
  ESP. Every `limine.conf` entry loads it after the slot's initramfs, and
  `get-myos --upgrade` and `--install --local` keep it (`docs/install.md`).
- The kernel unpacks it into the same tree (`[ OK ] tests: <n> files` at
  boot) and shows it as `/proc/boot/tests`.
- It holds the runner and every test script (`/lib/myos-tests`), the
  programs the ports name in `PORT_TEST_FILES` (`docs/ports.md`) and the
  Linux layer's test programs (`docs/linux-compat.md`).

### The runner

The runner is `/lib/myos-tests/run.sh` (from `user/tests`). After login
the launcher types

```sh
sh /lib/myos-tests/run.sh [--tags TAG,...] [--skip TAG,...]
```

Anyone can type this at the prompt too. The runner sources `kernel.sh`,
then the ports' scripts, `ports/<group>-<name>.sh` in name order: group
`0` for a core image port, `1` for another image port, `2` for a package.
It reads every test's tags before it runs any, and when a `pkg` test is
selected, its first test installs the packages the selected tests name
from the host's mirror, as apps in `/tmp/apps` (`docs/packages.md`).

One test is a command, usually a shell function, and its exit status
decides. Its output goes to `/tmp/myos-tests/<name>.out` and is printed
only when it fails, so the console shows one line per test:

```
tests: tags=- skip=net,pkg,heavy,install
TEST shell_echo PASS
TEST heap FAIL (exit 1)
    smoke start
    ...
HOST tests tcp-ping 2323
TESTS DONE 17/18
TESTS SKIPPED 45
```

## On the host

`src/boot_test.rs` runs the boot and `src/main.rs` sets up the machine:
its IOMMU (`docs/iommu.md`; `MYOS_X86_IOMMU=amd` for AMD's), the disks,
the network, the simulated Wi-Fi (`docs/wifi.md`) and, unless `net` is
skipped, the package mirror and the test web server. The boot disk is a copy of the image
(`target/boot-test-<name>.img`), so a test that writes to it leaves the
build's image as built.

The launcher logs in at `login: ` on the serial port, types the command,
then watches:

- the `TEST`, `HOST` and `TESTS DONE` lines;
- the kernel's crash reports (`exception:`, `user panic`, a user fault,
  a CPU stuck in the kernel, `docs/scheduler.md`), which end the run at
  once; a stuck CPU's ends it 3 s later, for the other CPUs' report
  lines, with every CPU's registers from the QEMU monitor;
- the stall watchdog and the budget (above);
- the kernel's **boot markers** (`[ OK ] heap`, `[ OK ] scheduler`, the
  drivers, the VFS checks of `/bin/custom/ok`), required whatever the
  tests say;
- after the tests, `poweroff`: QEMU must exit by itself (`docs/power.md`);
- then `e2fsck -fn` on the scratch disk (`target/scratch.img`) and
  `fsck.fat -n` on the FAT test disk (`target/fat.img`), each skipped when
  its tool is missing.

It exits 0 only when every test passed, every marker was seen and the
machine powered off. The full serial output is in the log either way.
`MYOS_NET_PCAP=<file>` makes QEMU write net0's frames, both ways, to that
pcap file (the boots after the tests to `<file>.2`, `<file>.3`): the
record of an exchange with the host that went wrong, a packet lost or
late (issue #656's mute connection). CI's boot jobs write one and keep a
failed boot's as the artifact `net0-pcap-<machine>`.

After a full list that passed, the launcher boots twice more:

1. the copy again, with `run.sh reboot`: the system must come up from
   slot `b` at the release the upgrade wrote, with the app the list
   installed in `/data/apps` still there;
2. the scratch disk that `get-myos --install --local` made (the list's
   last test, kept as `target/installed-<name>.img`), with `run.sh
   installed`: it must come up from its slot `a`.

## In CI

### What a run boots

`ci-runtime.yml` boots nothing fixed. Its `plan` job runs
`scripts/ci-select.py`, which reads every test's tags and the paths the
change touches, and writes the boot job's matrix. `scripts/ci-select.py
--changed PATH...` prints the plan locally for any set of paths, and
`--all` prints the full one; the rules are in its source. In short:

| changed | selects |
|---|---|
| a port's or a package's directory | its tests, those of the ports built on it or needing it, and what it implements (netd: `netfs`; get-myos: `install`, `pkg`) |
| `modules/<name>` | the module's tag; `modules/abi`, `acpi` and `pci_enum`: everything |
| `linux-compat/` | `linux` |
| `kernel/src/arch/<arch>` | everything on that arch's machines, the long groups included; `smoke` at least elsewhere |
| `kernel/src/iommu/`, `kernel/src/limine_boot.rs`, `src/`, `boot/`, `.github/`, `scripts/ci-*` | everything on every machine: what differs between an arch's machines |
| `kernel/`, `toolchain/`, `build.rs`, `scripts/`, `etc/`, `user/init`, `user/tests`, anything else | everything on the primary machines, `smoke` on the variants |
| documentation (`*.md`, `docs/`, `licenses/`) | nothing: no boot |

`smoke` is always in. A push to master, the daily run, a dispatch of
`ci.yml` with `full_boot: true` and a pull request labelled `ci:full` run
everything on every machine. The image carries the Linux layer
(`MYOS_CI_FEATURES=linux_compat`) when a `linux` test runs.

### Machines

| machine | what sets it apart |
|---|---|
| bios | x86 by BIOS |
| uefi | x86 by UEFI; the x86 primary |
| `bios-amd` | bios with AMD's IOMMU; its light boot only |
| aarch64 | GICv2 |
| aarch64-gicv3 | GICv3; the aarch64 primary |
| riscv64 | QEMU's AIA (the APLIC and the IMSICs, MSIs); the riscv64 primary |
| `riscv64-plic` | the PLIC (`MYOS_RISCV_INTC=plic`), INTx only; its light boot only |

A pull request's selection boots on each arch's primary machine. The
variants boot the smoke set for a change that selects everything, and
nothing for one that selects tags. The x86 boots run under KVM, the
others under TCG.

### Boots per machine

A machine's selection runs in parallel boots. Each long group runs whole,
in a boot of its own: `ostest` (the curated os-test list), `alpine` (the
apk installs), `rust` (the rust package's tests) and `pkg` (the packages'
install, their tests and get-myos's, and the `install` tests). One light
boot runs everything else. A full run is thus five boots per machine.

On aarch64 and riscv64, under TCG, the long groups take two to three
times what the light boot does (16 minutes against 9 on riscv64). So a
pull request boots only the light tests there, and the groups on x86
alone, under KVM. Paths whose breakage the groups have caught on one arch
before boot the groups on aarch64 and riscv64 too:

`user/get-myos`, `linux-compat/`, `packages/os-test` and
`kernel/src/arch/<arch>`. The runs of everything boot the groups on every
machine.

The Alpine disk is restored from the Actions cache, keyed by
`linux-compat/alpine-disks.sh --hash`, or made on a miss. Bump `FIXTURE`
in that script for a fresh disk, since Alpine's latest-stable moves.

### Raspberry Pi 3

The Raspberry Pi 3 (QEMU `raspi3b`, `myos rpi3 test`) boots once whenever
an aarch64 machine does: the `smoke` tests it can run and the USB tests
of its devices. It has no disk but its SD card, and no network or PCI,
so the tests that need the scratch disk, the network, another pointer or
a USB root port of their own are skipped (the list is in
`scripts/ci-select.py`; `user/c-mlibc/test.sh` explains why the mlibc
twins follow). Its boot checks the markers it can print
(`boot_test::Board::Rpi3`, `docs/raspberry-pi.md`).
