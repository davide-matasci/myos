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
instead:

- QEMU runs with `-accel kvm`.
- The interrupt controllers are split between KVM and QEMU, for the
  IOMMU's interrupt remapping.
- The CPU model stays QEMU's `qemu64`, so the guest sees the same CPU
  either way.

The light list then takes about 400 s instead of 670 to 950. CI does this
for its x86 boots ("In CI", below). aarch64 and riscv64 stay under TCG on
an x86 host.

## Selecting tests

Every test carries **tags**: what it needs, what it covers, and its own
name. `test` takes the tests with any of the `--tags` (all tests without
`--tags`), minus those with any of the `--skip` tags. So `--tags dns` runs
that one test, and `--skip heap` leaves that one out. `test-mini` and
`test-full` are the two lists of old: `test --skip
net,pkg,heavy,install,alpine-disk` and `test`.

| tag | the tests that |
|---|---|
| `net` | need the host's network: the package mirror and the test web server the launcher serves, GitHub, the Alpine mirror. Without it a run stays on QEMU's user network; DNS and the listen test go through it. |
| `pkg` | need the packages of this build. Either installed as apps (`packages`, the first test of a run that has one, installs from the mirror those the selected `pkg` tests name in their tags; every package's own test has `pkg` and its package's name), or the mirror itself (get-myos's tests, which name none). In CI, the tests of the `pkg` boot. |
| `heavy` | take minutes under TCG: the curated os-test list, the Alpine apk installs (jq, netpbm), the heavy smoke with the apps |
| `alpine-disk` | mount the Alpine roots the host prepares: rustc, Python, an X client (`linux-compat/alpine-disks.sh`, fetched once into `target/alpine-<arch>.img`, attached as `/dev/nvme2n1/data`) |
| `install` | change the boot disk: `get-myos --upgrade` (with Limine made stale first) and `--install` on the scratch disk. When they pass, the launcher boots the upgraded disk again, which must come up from slot `b`, and then the disk `--install --local` made, from its slot a (`docs/install.md`). |
| `smoke` | are the quick basics: the shell, exec, the basic programs, the filesystems on the scratch disk, `heap mini`; about two minutes |
| a module's name (`netfs`, `console`, `linux`, `ext2`, `fat`, `usb`, `blk`, `virtio_net`, ...) or an area's (`sec`, `tty`, `events`, `mm`, `sched`, `proc`, `fs`, `exec`, `x11`, `pci`) | cover it: what a change there must run |
| a port's name (`oksh`, `c-smokes`, `curl`, `os-test`, ...) | come from that port's `test.sh`. The runner sets it on every test of the file. |
| `mlibc` | run a C smoke built against mlibc instead of newlib (`user/c-mlibc/test.sh`, the `mlibc_` tests). Each is also tagged with its newlib test's name (`docs/mlibc.md`). |
| `alpine`, `python`, `rust`, `x11`, `ostest` | the long tests, by what they build, for a run of one of them alone |

### Budgets and hung tests

- A quick run (`heavy` and `net` skipped) has a budget of 8 minutes, 9
  with the Linux layer. riscv64, the slowest under TCG, gets half as much
  again, as in CI. Its stall limit is 3 minutes.
- Any other run has a budget of 50 minutes and a stall limit of 10.
- `--budget MINUTES` sets the budget.
- The runner prints what it skipped (`TESTS SKIPPED <n>`) after
  `TESTS DONE`.
- A test still running after 2 minutes (5 when `heavy` is not skipped,
  inside either stall limit) gets a `HUNG <name>` line on the console.
  After it, once, come every thread's `/proc/<pid>/task/<tid>/status`
  line and every process's command line. The status line shows the
  thread's state and, in `wait`, the key it is blocked on (`docs/proc.md`;
  `kernel/src/task/sched.rs` lists the key ranges: a pipe, a pty, a
  child, ...). So a test that hangs shows what waits on what before the
  host gives up.

## In the guest

### The tests module

The tests are not in the image. The ISO and the release's boot files
(what `get-myos --install` and `--upgrade` write) carry none of them.
They are the **tests module**: a second cpio archive beside the
initramfs, a zstd frame too, decoded at boot like it.

- Only the build's disks carry it (`target/*.img`, what the boot tests
  boot). It is `boot/tests` on the ESP, outside the slots, and every entry
  of the disk's `limine.conf` loads it after the slot's initramfs.
- The kernel unpacks it into the same tree (`[ OK ] tests: <n> files` at
  boot) and shows it as `/proc/boot/tests`.
- It holds the runner and every test script (`/lib/myos-tests`), the
  programs the ports name in `PORT_TEST_FILES` (`docs/ports.md`: the C
  smokes under `/bin/etc` and `/bin/etc/mlibc`) and the Linux layer's
  test programs (`/bin/linux`, `docs/linux-compat.md`).
- `src/initramfs.rs` (`build_boot_archives`) splits the two archives.
- A test boot's upgrade keeps the module: `get-myos` writes the line into
  the entries it makes while the ESP has the file. `get-myos --install
  --local` copies it (`docs/install.md`). So the boots after those find
  the runner too.

### The runner

The runner is `/lib/myos-tests/run.sh` (from `user/tests`). After login
the launcher types

```sh
sh /lib/myos-tests/run.sh [--tags TAG,...] [--skip TAG,...]
```

Anyone can type this at the prompt too (`mini` and `full` are the two
lists of old). One test is a command, usually a shell function, and its
exit status decides. Its output goes to `/tmp/myos-tests/<name>.out` and
is printed only when it fails, so the console shows one line per test:

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

- `TEST <name> PASS|FAIL`: one per test.
- `HOST <port> <args>`: the test needs the host. The launcher runs that
  port's host-side script (`PORT_HOST=host.sh` in its `port.env`, from
  the checkout) with the arguments, in the background. The guest test
  then waits for what the script does. A port that needs a peer on the
  host ships its own `host.sh`; the launcher knows no test. The host
  scripts in the tree:
  - `c-smokes tcp-ping PORT TOKEN` (`user/c/host.sh`) connects to the
    guest's listener through QEMU's port forward and plays ping/pong.
    Then it opens and closes forty connections without a word. The guest
    asks for it once `/net/convs` names the listener. `tcp-ping-stop PORT
    TOKEN` ends a peer whose smoke failed, so that it does not retry onto
    the next listen test's listener.
  - `tests usb-plug|usb-unplug` (`user/tests/host.sh`) plugs a second USB
    stick into the guest through the QEMU monitor and pulls it out
    (`docs/usb.md`).
  - `dropbear PORT` (`ports/dropbear/host.sh`) opens two SSH sessions at
    once with the test key to the dropbear the boot script started
    (`docs/ssh.md`). Each session touches `/tmp/ssh-ok-a` / `-b` for the
    guest test to find. Then it logs in on a pty (`ssh -tt`) and writes
    its terminal to `/tmp/ssh-tty`.
  - `acme serve` (`packages/acme/host.sh`) runs the acme package's test
    ACME server (`acme-server.py`, Python and the host's openssl) on port
    14000. It validates the http-01 challenge against nginx in the guest
    through the forward of port 5002, until the guest test asks it to
    stop.
- `TESTS DONE <passed>/<total>` ends the run. `TESTS SKIPPED <n>` after it
  says how many tests the selection left out.

### Where the tests live

Every test belongs to the port of what it tests and ships with it
(`test.sh`, below):

- the shell's tests are oksh's;
- `heap`'s are `user/heap`'s;
- the ext2 ones are `mkfs.ext2`'s;
- DNS and HTTPS are the `dns` and `http` programs';
- listen/accept and the tty are the C smokes'.

`user/tests` has one section of its own, `kernel.sh`, for what has no
port directory: the exec limits and the Linux layer. With the feature in
the image it runs the layer's smokes; without it, it checks that the boot
script's `rc.local` loaded the layer with `insmod`.

The runner sources `kernel.sh`, then `ports/*.sh` in name order. The
packer names those files so that the core image ports (the shell, the
basic programs) come first, the other image ports next and the packages
last. The image carries the packages' test files too, so the runner reads
every test's tags before it installs anything.

When a selected test has `pkg`, the very first test installs the packages
the selected tests use from the host's mirror, as apps in `/tmp/apps`
(`MYOS_APPS`, `docs/packages.md`). A `pkg` test names in its tags the
packages it runs: its own package's tests carry its name, `alpine_jq`
names `get-alpine`, the X clients `tinyx`. A test that names none gets
none (get-myos's own tests install into roots of their own). The tests
run their programs with `run-myos`.

### The test web server

The tests that fetch over HTTP(S) use the launcher's test web server
(`src/test_www.rs`), not a site on the internet, whose answers vary from
run to run (a rate limit, a 503).

- Plain HTTP is on the host's port 8766, HTTPS on 8767: the guest's
  `10.0.2.2:8766` and `10.0.2.2:8767`.
- It serves `/hello.txt`.
- HTTPS presents a certificate of the test CA (`user/tests/tls`, made by
  its `make.sh` and committed, like dropbear's test key).
- The tests trust that CA alone (`/lib/myos-tests/test-ca.pem`: curl's
  `--cacert`, Python's `cafile`), never the image's bundle.

What needs the real internet uses GitHub, which CI cannot run without
anyway. `dns` and the Linux layer's lookups resolve `github.com`. The
kernel's own HTTPS client (`https`) fetches `https://github.com/robots.txt`
against the image's bundle.

The tty test (`user/c/tty_smoke.c`, in `user/c/test.sh`) drives an
interactive shell on a pty the way a person types at the console: history
recall and in-line editing with the arrow keys, backspace, and `^C` on a
foreground pipeline with the shell surviving.

### A port's own tests

A port ships its tests with `PORT_TEST=test.sh` in its `port.env`
(`docs/ports.md`). The file lands at
`/lib/myos-tests/ports/<group>-<name>.sh`, in the tests module or in the
package's app. The group is `0` for a core image port, `1` for another
image port and `2` for a package, so moving a port between `ports/` and
`packages/` moves its test too. The runner sources every file of that
directory in name order, after `kernel.sh`. A test script calls `t`:

```sh
# packages/make/test.sh
make_version() {
	run-myos make --version | grep -q "GNU Make" && [ ! -e /bin/custom/make ]
}
t make make_version
```

- `t -t TAG,TAG name function` tags the test: what it needs (`net`,
  `pkg`, `heavy`, `install`) and the module or area it covers. The port's
  own name is on every test of its file already, and `pkg` on every test
  of a package.
- `$OUT` is the output directory.
- `contains NEEDLE FILE` greps.
- fd 3 is the console (`echo "HOST ..." >&3`). A long test can stream its
  progress there, as os-test does.
- Keep the output of a passing test to itself. Failures show the last 40
  lines.

A test that needs a peer outside the guest (a client connecting in) gets
one from the port's own `host.sh` (`PORT_HOST=host.sh`):

1. The guest side prints `HOST <port> <args>` to fd 3.
2. The launcher runs `bash <port dir>/host.sh <args>` on the host.
3. The guest side waits, with a bound, for the effect: a file the session
   touched, a reply on its socket.

The script's messages go to its stderr. Its exit status is logged, but
the guest test decides. A test boot's QEMU monitor is a unix socket the
scripts find in `$MYOS_QEMU_MONITOR`:

- `user/c/host.sh sendkey KEYS...` types on the guest's keyboard through
  it (`/dev/console/kbd`), the keys in order. Several `HOST` requests run
  in parallel, so one request per key would not keep the order.
- `user/c/host.sh mouse` moves and clicks the test boots' `usb-mouse`
  (`/dev/console/mouse`), and back.
- `mouse ps2` and `mouse virtio` do the same with QEMU's PS/2 mouse (x86)
  and virtio mouse (the `virt` boards' test boots carry one) through
  `mouse_set`.

## On the host

`src/boot_test.rs` starts QEMU with the serial console on its stdio.
`src/main.rs` sets up:

- the machine: on x86 `q35` with Intel's IOMMU, or AMD's under
  `MYOS_X86_IOMMU=amd`; on aarch64 `virt` with an SMMUv3; on riscv64
  `virt` with the RISC-V IOMMU where QEMU is 9.2 or later
  (`docs/iommu.md`);
- the disks;
- the network with the port forwards;
- the simulated Wi-Fi radio and its access points (`src/wifi_sim.rs`,
  `docs/wifi.md`);
- unless `net` is skipped, the package mirror and the test web server;
- with the Linux layer and unless `alpine-disk` is skipped, the Alpine
  disk. `linux-compat/alpine-disks.sh` builds it once into
  `target/alpine-<arch>.img`: the roots of rustc, Python and an X client,
  from `get-alpine -n` on the host. It is attached as `/dev/nvme2n1/data`,
  with its writes kept in a QEMU snapshot.

The launcher then waits for `login: ` on the serial port (`ttyS0`'s
getty, `docs/tty.md`). A kernel status line that lands after a prompt is
skipped. It types `root`, an empty password, `COLUMNS=1000` and the
command. It types each byte once the echo of the last is back, so an AP's
lagging echo never garbles the line. The `COLUMNS` line keeps the shell's
line editor from scrolling a long command sideways, as it would on the
`virt` boards' 100-column console. Then it watches:

- the `TEST`, `HOST` (the named port's `host.sh`) and `TESTS DONE` lines;
- the kernel's crash reports (`exception:`, `user panic`, a user fault),
  which end the run at once;
- a **stall watchdog**: the console must print something within 3 minutes
  (a quick run) or 10 minutes, and the whole run has its budget. A stall
  ends with every CPU's registers from the QEMU monitor (`info registers
  -a`), which show, by `addr2line` on the kernel, where the guest spins
  or halts;
- the kernel's own **boot markers** (`[ OK ] heap`, `[ OK ] dma pages`,
  `[ OK ] scheduler`, the drivers, the VFS checks of `/bin/custom/ok`),
  which init prints before the login prompt. They are required whatever
  the tests say. `ok` runs on every boot, so it writes to a disk
  (`mkfs.ext2` on `/dev/nvme0n1/data`) and expects the FAT mount only once
  it has found the launcher's FAT volume (`/msg` reading `fat-msg`). In
  another VM it leaves the disks alone;
- after the tests, `poweroff` typed at the prompt. QEMU must exit by
  itself, with the console naming the method (`power off via ...`,
  `docs/power.md`);
- then `e2fsck -fn` on the scratch disk (`target/scratch.img`, the
  guest's `/dev/nvme1n1/data`) when the ext2 tests left a filesystem on
  it, and `fsck.fat -n` on the FAT test disk (`target/fat.img`, the
  guest's `/dev/vda/data`, which `fat_rw` writes to). Each check is
  skipped when its tool (e2fsprogs, dosfstools) is missing.

It exits 0 only when every test passed, every marker was seen and the
machine powered off. It prints a one-line summary with the failed tests'
names. The full serial output is in the log either way.

The boot disk is a copy of the image (`target/boot-test-<name>.img`), so a
test that writes to it leaves the build's image as built. myos sees it on
every arch as a virtio disk. On x86 it is a third legacy virtio-blk,
`vdc`, after the test disks, which keep their names. It is not NVMe,
because SeaBIOS cannot reach NVMe's 64-bit BAR with 4 GiB of RAM.

After a full list that passed, the launcher boots twice more:

1. It boots the copy again and types `sh /lib/myos-tests/run.sh reboot`.
   This checks only that the system came up from slot `b` at the release
   the upgrade wrote, and that the app the list installed in `/data/apps`
   (on the boot disk's data partition) is still there and runs.
2. It boots, as the boot disk, the scratch disk that
   `get-myos --install --local` made. That is the list's last test,
   `t_last` in `run.sh`, after mkfs.ext2's tests have formatted the whole
   scratch disk; the disk is kept as `target/installed-<name>.img`. It
   boots by BIOS on the bios job and by UEFI on the others, and
   `run.sh installed` checks that it came up from its slot `a`.

## In CI

### What a run boots

`ci-runtime.yml` boots nothing fixed. Its `plan` job runs
`scripts/ci-select.py`, which reads every test's tags (the `t -t` lines,
and the ports' names from the descriptors) and the paths the change
touches, and writes the boot job's matrix. A path selects tags:

| changed | selects |
|---|---|
| a port's or a package's directory | its tests (its name), those of the ports built on it or needing it (`PORT_DEPS`, `PORT_RDEPS`) and what it implements (netd: `netfs`; mkfs.ext2: `ext2`; get-myos: `install`, `pkg`) |
| `modules/<name>` | the module's tag (console: `console`, `tty`; the disk drivers: `blk`, sdmmc also `sdmmc` and `sdio`; the USB modules: `usb`, usb_audio, uhci and ohci also `sound`; virtio_sound: `sound`; virtio_net: `virtio_net`, `netfs`; pl061: `irq`); `modules/abi`, `acpi` and `pci_enum`: everything |
| `linux-compat/` | `linux` |
| `kernel/src/arch/<arch>` | everything on that arch's machines, variants too, the long groups included; the tags (`smoke` at least) on the other arches' primary machines |
| `kernel/src/iommu/`, `kernel/src/limine_boot.rs`, `src/`, `boot/`, `.github/`, `scripts/ci-*` | everything on every machine: what differs between an arch's machines |
| `user/get-myos`, `linux-compat/`, `packages/os-test` | their tags, with the long groups on aarch64 and riscv64 too (below) |
| `kernel/`, `toolchain/`, `build.rs`, `scripts/`, `etc/`, `user/init`, `user/tests`, anything else | everything on the primary machines, `smoke` on the variants |
| documentation (`*.md`, `docs/`, `licenses/`) | nothing: no boot |

`smoke` is always in. A push to master, the daily run, a dispatch of
`ci.yml` with `full_boot: true` and a pull request labelled `ci:full` run
everything on every machine; a full run also installs the build's own
packages into the guest with `get-myos`, from a mirror the launcher
serves. The image carries the Linux layer (`linux_compat`, through
`MYOS_CI_FEATURES=linux_compat`) when a `linux` test runs.

The plan is in the run's summary. `scripts/ci-select.py --changed PATH...`
prints it locally for any set of paths, and `--all` prints the
full one.

### Machines

| machine | what sets it apart |
|---|---|
| bios | x86 by BIOS |
| uefi | x86 by UEFI; the x86 primary |
| `bios-amd` | bios with AMD's IOMMU; its light boot only, since the long groups exercise nothing the unit changes |
| aarch64 | GICv2 |
| aarch64-gicv3 | GICv3; the aarch64 primary |
| riscv64 | QEMU's AIA: the APLIC and the IMSICs, the PCI devices' MSIs; the riscv64 primary |
| `riscv64-plic` | the PLIC (`MYOS_RISCV_INTC=plic`): INTx only; its light boot only, like `bios-amd` |

The Raspberry Pi 3 (below) runs its own boot.

Each arch has a primary machine (uefi, aarch64-gicv3, riscv64): the one a
pull request's selection boots on. The others are variants of it, in the
firmware, the IOMMU or the interrupt controller. They boot the smoke set
for a change that selects everything, and nothing for one that selects
tags. What sets them apart is exercised by the machine-specific paths of
the table (everything on every machine) and by master's runs.

The x86 boots (bios, uefi, bios-amd) run under KVM. The plan says so
(`accel`), the boot job's container takes the runner's `/dev/kvm`
(GitHub's Linux runners have it), and the launcher boots with
`-accel kvm` as in "Running the tests". The other arches run under TCG,
on x86 runners.

### Boots per machine

A machine's selection runs in parallel boots. Each long group runs whole,
in a boot of its own:

- `ostest`: the curated os-test list;
- `alpine`: the apk installs, whose tests build on each other;
- `rust`: the rust package's tests, which set the toolchain up in the
  guest (a download of some 230 MB) and build with it under the Linux
  layer; 11 minutes under TCG;
- `pkg`: the packages' install from the mirror, the packages' and
  get-myos's tests on it, and the `install` tests of the boot disk; a
  boot of 13 minutes on riscv64, which the light boot no longer carries.

One light boot runs everything else (9 minutes on riscv64). A full run is
thus five boots per machine instead of one.

Each boot gets a budget from what it runs (riscv64 half as much again).
A selection without `net` tests skips the mirror. One without
`alpine-disk` tests skips the Alpine disk. A boot that needs the disk
restores it from the Actions cache, keyed by
`linux-compat/alpine-disks.sh --hash` (the scripts that make it), or
makes it on a miss. Bump `FIXTURE` in that script for a fresh disk, since
Alpine's latest-stable moves.

On aarch64 and riscv64, which run under TCG, the long groups take two to
three times what the light boot does (16 minutes against 9 on riscv64).
So a pull request boots only the light tests there, whatever it selects,
and the groups on x86, under KVM, where they take 3 to 6 minutes. These
paths boot the groups on aarch64 and riscv64 in a pull request too,
because the groups have caught them breaking one arch before:

- `user/get-myos`: riscv64's lseek, brk heap and ESP broke the install
  and the upgrade;
- `linux-compat/`: musl's vfork on aarch64 and riscv64;
- `packages/os-test`: its semantics have per-arch issues open;
- `kernel/src/arch/<arch>`, which boots everything on its arch.

The runs of everything (master, daily, `ci:full`) boot the groups on
every machine, so a port's or the kernel's arch-specific breakage in them
is found within the hour of merging. A pull request that touches the
kernel thus boots 10 times instead of 16, its longest boot the riscv64
light one. One that touches a package boots 5 times, four of them smoke
boots.

### Raspberry Pi 3

The Raspberry Pi 3 (QEMU `raspi3b`, `myos rpi3 test`) boots once whenever
an aarch64 machine does, with a fixed selection:

- the `smoke` tests it can run;
- the USB tests of its devices (`usb_disk`, `mouse`, `usb_keyboard`,
  `usbdev`): QEMU's hub on its one root port, with the stick, the mouse
  and the serial adapter behind it, and the keyboard the test plugs in.
  The host scripts take the bus and the hub's port from `MYOS_USB_BUS`
  and `MYOS_USB_PORT`, which the launcher sets per board.

It has no disk but the SD card it boots from (`sdmmc` checks it, on the
sdhost), and no network or PCI. So these tests are skipped:

- those that need the scratch disk: `partitions`, `ext2_disk`,
  `fstab_mount`, `fileio`;
- those that need the network: `boot_rc` (the rc.local's dropbear) and
  `heap_mini` (its ping and socket stages);
- those that need another pointer: `ps2_mouse`, `virtio_mouse`,
  `xev_pointer`;
- those that need a root port of their own: `usb_hotplug`,
  `usb_hotplug_busy_reuse`, `usb_hub_replug`.

Its boot checks the markers it can print (`boot_test::Board::Rpi3`). The
build job builds its boot files (`boot/rpi3/build.sh`, with U-Boot cached
by its pin) and packs them with the rest.

### Caches

- GitHub Actions caches Cargo with Swatinem/rust-cache (`prefix-key: limine-8.3-6`).
- Userspace port outputs are OCI artifacts on GHCR, one package per port
  (its `PORT_OUTPUTS`, `scripts/ports.sh`), tagged with the stamp hash
  from `scripts/myos-c-userspace-lib.sh`. The ports matrices come from
  the descriptors. The first run after a stamp changes misses and pushes;
  later runs with the same hashes hit. The GHCR packages should be
  **public** for pull requests from forks.
- Source checkouts (`*-src`) are never cached.
- rust-cache restores `target/` with the Rust ports' cargo target
  directories in it. Cargo does not track the std sysroot, newlib or
  target spec they were built against, so each such directory carries a
  stamp of them and starts over when it differs (`myos_cargo_target_dir`,
  `toolchain/std/lib.sh`).

On master, the x86_64 hybrid ISO with the Linux layer in it
(`--features linux_compat`) and the packages go to the rolling `rolling`
release, get-myos's default mirror (`docs/packages.md`).

## Adding a test

- A port's or a program's behaviour: a function and a `t -t tags name
  function` line in the `test.sh` next to its `port.env` (`PORT_TEST`).
  Add a `host.sh` with `PORT_HOST` when the test needs a peer on the
  host. Programs the tests need (smokes) go in `user/c` or as a `user`
  crate, with their test.
- The kernel's own behaviour (a limit, a module without a user-side
  program): `user/tests/kernel.sh`, tagged with the module or area.
- A POSIX conformance case: the curated os-test lists
  (`packages/os-test/overlay/misc/*.tests`), which must pass in full.

Tag what the test needs (`net`, `pkg`, `heavy`, `install`) and what it
covers (the module's or area's name). The needs decide which runs it can
be in; the coverage decides which changes must run it.

Test on every arch the change could affect; arch-specific code needs all
three. A quick run per arch is usually enough locally, and CI does the
rest. The `net` tests need the network. In a sandbox behind a
TLS-intercepting proxy, append its CA to `target/cacert.pem` for local
runs only, and restore it afterwards.

## History

- The `HUNG` report exists because CI #2295's lynx test stalled with
  every CPU idle, and the console showed nothing more.
