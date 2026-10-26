# Boot tests

The real test of myos is booting it. The launcher boots an image headless
in QEMU, logs in and runs a test list **in the guest**; the host only
watches the serial console for the result.

```sh
cargo run -- test                    # x86_64 BIOS, every test (needs the network)
cargo run -- aarch64 test            # also uefi, riscv64
cargo run -- test --skip net,pkg,heavy,install,alpine-disk   # the quick list: no network, a few minutes (`test-mini`)
cargo run -- test --tags smoke       # the smoke set alone
cargo run -- test --tags ssh,https   # these two: a test's name is a tag of its own
scripts/local-ci.sh [bios|uefi|aarch64|riscv64] [mini|full|test args...]   # the same, with the OOM/TCG settings for a loaded host
```

A boot without `test` is a normal boot to the login prompt (what the ISO
is for).

QEMU emulates the guest's CPU (TCG) unless told otherwise. On a host with
`/dev/kvm`, `MYOS_X86_ACCEL=kvm` runs the x86 boots on the host's CPU
instead (`-accel kvm`, the interrupt controllers split between KVM and
QEMU for the IOMMU's interrupt remapping; the CPU model stays QEMU's
`qemu64`, so the guest sees the same CPU either way): the light list
takes about 400 s instead of 670 to 950. CI does so for its x86 boots
(below); aarch64 and riscv64 stay under TCG on an x86 host.

## Selecting tests

Every test carries **tags**: what it needs, what it covers, and its own
name. `test` takes the tests with any of the `--tags` (all of them
without), minus those with any of the `--skip` tags; so `--tags dns` runs
that one test and `--skip heap` leaves that one out.
`test-mini` and `test-full` are the two lists of old: `test --skip
net,pkg,heavy,install,alpine-disk` and `test`.

| tag | the tests that |
|---|---|
| `net` | need the host's network: the package mirror and the test web server the launcher serves, GitHub, the Alpine mirror (without it a run stays on QEMU's user network: DNS and the listen test go through it) |
| `pkg` | need the packages of this build: installed as apps (`packages`, the first test of a run that has one, installs from the mirror those the selected `pkg` tests name in their tags; every package's own test has it and its package's name), or the mirror itself (get-myos's, which name none); in CI the `pkg` boot's |
| `heavy` | take minutes under TCG: the curated os-test list, the Alpine apk installs (jq, netpbm), the heavy smoke with the apps |
| `alpine-disk` | mount the Alpine roots the host prepares (rustc, Python, an X client: `linux-compat/alpine-disks.sh`, fetched once into `target/alpine-<arch>.img`, attached as `/dev/nvme2n1/data`) |
| `install` | change the boot disk: `get-myos --upgrade` (Limine made stale first) and `--install` on the scratch disk; when they passed, the launcher boots the upgraded disk again, which must come up from slot `b`, and then the disk `--install --local` made, from its slot a (`docs/install.md`) |
| `smoke` | are the quick basics: the shell, exec, the basic programs, the filesystems on the scratch disk, `heap mini`; about two minutes |
| a module's name (`netfs`, `console`, `linux`, `ext2`, `fat`, `usb`, `blk`, `virtio_net`, ...) or an area's (`sec`, `tty`, `events`, `mm`, `sched`, `proc`, `fs`, `exec`, `x11`, `pci`) | cover it: what a change there must run |
| a port's name (`oksh`, `c-smokes`, `curl`, `os-test`, ...) | come from that port: its `test.sh` (every test of the file, set by the runner) |
| `mlibc` | run a C smoke built against mlibc instead of newlib (`user/c-mlibc/test.sh`, the `mlibc_` tests, each also tagged with its newlib test's name: `docs/mlibc.md`) |
| `alpine`, `python`, `rust`, `x11`, `ostest` | the long tests by what they build, for a run of one of them alone |

A quick run (`heavy` and `net` skipped) has a budget of 6 minutes (7 with
the Linux layer; half as much again on riscv64, the slowest under TCG, as
in CI) and a stall limit of 3; any other 50 and 10
(`--budget MINUTES` sets the budget). The runner prints what it skipped
(`TESTS SKIPPED <n>`) after `TESTS DONE`.

## In the guest

The tests are not in the image: the ISO and the release's boot files
(what `get-myos --install` and `--upgrade` write) carry none of them. They
are the **tests module**, a second cpio archive beside the initramfs (a
zstd frame too, decoded at boot like it) that
only the build's disks carry (`target/*.img`, what the boot tests boot):
`boot/tests` on the ESP, outside the slots, which every entry of the
disk's `limine.conf` loads after the slot's initramfs. The kernel unpacks
it into the same tree (`[ OK ] tests: <n> files` at boot) and shows it as
`/proc/boot/tests`. It holds the runner and every test script
(`/lib/myos-tests`), the programs the ports name in `PORT_TEST_FILES`
(`docs/ports.md`: the C smokes under `/bin/etc` and `/bin/etc/mlibc`) and
the Linux layer's test programs (`/bin/linux`, `docs/linux-compat.md`);
`src/initramfs.rs` (`build_boot_archives`) splits the two. A test boot's
upgrade keeps it (`get-myos` writes the line into the entries it makes
while the ESP has the file) and `get-myos --install --local` copies it
(`docs/install.md`), so the boots after those find the runner too.

The runner is `/lib/myos-tests/run.sh` (from `user/tests`); after login
the launcher types

```sh
sh /lib/myos-tests/run.sh [--tags TAG,...] [--skip TAG,...]
```

which anyone can type at the prompt too (`mini` and `full` are the two
lists of old). One test is a command, usually a shell function; its exit
status decides. Its output goes to `/tmp/myos-tests/<name>.out` and is
printed only when it fails, so the console shows one line per test:

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
  port's host-side script (`PORT_HOST=host.sh` in its `port.env`, from the
  checkout) with the arguments, in the background; the guest test then
  waits for what the script does. `c-smokes tcp-ping PORT TOKEN` (`user/c/host.sh`)
  connects to the guest's listener through QEMU's port forward, plays
  ping/pong and then opens and closes forty connections without a word
  (asked for once `/net/convs` names the listener; `tcp-ping-stop PORT
  TOKEN` ends a peer whose smoke failed, so it does not retry onto the
  next listen test's); `tests usb-plug|usb-unplug` (`user/tests/host.sh`) plugs a
  second USB stick into the guest through the QEMU monitor and pulls it out
  (`docs/usb.md`); `dropbear PORT` (`ports/dropbear/host.sh`) opens two SSH
  sessions at once with the test key to the dropbear the boot script
  started (`docs/ssh.md`), each of which touches
  `/tmp/ssh-ok-a` / `-b` for the guest test to find, then a login on a pty
  (`ssh -tt`) that writes its terminal to `/tmp/ssh-tty`; `acme serve`
  (`packages/acme/host.sh`) runs the acme package's test ACME server
  (`acme-server.py`, Python and the host's openssl) on port 14000, which
  validates the http-01 challenge against nginx in the guest through the
  forward of port 5002, until the guest test asks it to stop. A port that
  needs a peer on the host ships its own `host.sh`; the launcher knows no
  test.
- `TESTS DONE <passed>/<total>` ends the run; `TESTS SKIPPED <n>` after it
  says how many the selection left out.

Every test belongs to the port of what it tests and ships with it
(`test.sh`, see below): the shell's are oksh's, `heap`'s are `user/heap`'s,
the ext2 ones are `mkfs.ext2`'s, DNS and HTTPS are the `dns` and `http`
programs', listen/accept and the tty are the C smokes'. The one section of
`user/tests` itself, `kernel.sh`, holds what has no port directory: the
exec limits and the Linux layer (its smokes when the image was built with
the feature; otherwise that the boot script's `rc.local` loaded it with
`insmod`). The runner sources `kernel.sh`, then
`ports/*.sh` in name order; the packer names those so the core image ports
(the shell, the basic programs) come first, the other image ports next,
the packages last (the image carries the packages' test files too, so
the runner reads every test's tags before it installs anything). When a
selected test has `pkg`, the very first test installs the packages the
selected tests use from the host's mirror, as apps in `/tmp/apps`
(`MYOS_APPS`, `docs/packages.md`): a `pkg` test names in its tags the
packages it runs (its own package's tests carry its name; `alpine_jq`
names `get-alpine`, the X clients `tinyx`); one that names none gets
none (get-myos's own install into roots of their own). The tests run
their programs with `run-myos`.

The tests that fetch over HTTP(S) do it from the launcher's test web
server (`src/test_www.rs`), not from a site on the internet, whose
answers vary from run to run (a rate limit, a 503): plain HTTP on the
host's port 8766 and HTTPS on 8767, the guest's `10.0.2.2:8766` and
`10.0.2.2:8767`. HTTPS presents a certificate of the test CA
(`user/tests/tls`, made by its `make.sh` and committed, like dropbear's
test key); the tests trust that CA alone (`/lib/myos-tests/test-ca.pem`:
curl's `--cacert`, Python's `cafile`), never the
image's bundle. It serves `/hello.txt`. What needs the real internet
uses GitHub, which CI cannot run without anyway: `dns` and the Linux
layer's lookups resolve `github.com`, and the kernel's own HTTPS client
(`https`) fetches `https://github.com/robots.txt` against the image's
bundle.

The tty test (`user/c/tty_smoke.c`, in `user/c/test.sh`) drives an
interactive shell on a pty the way a person types at the console: history
recall and in-line editing with the arrow keys, backspace, `^C` on a
foreground pipeline with the shell surviving.

### A port's own tests

A port ships its test with `PORT_TEST=test.sh` in its `port.env`
(`docs/ports.md`): the file lands at `/lib/myos-tests/ports/<group>-<name>.sh`
in the tests module, or in the package's app (`0` for a core image port, `1` for
another image port, `2` for a package), so moving a port between `ports/` and
`packages/` moves its test too. The runner sources every file of that
directory in name order, after `kernel.sh`. A test script calls `t`:

```sh
# packages/make/test.sh
make_version() {
	run-myos make --version | grep -q "GNU Make" && [ ! -e /bin/custom/make ]
}
t make make_version
```

`t -t TAG,TAG name function` tags the test (above: what it needs, `net`,
`pkg`, `heavy`, `install`, and the module or area it covers); the port's
own name is on every test of its file already, `pkg` on a package's.
`$OUT` is the output directory, `contains NEEDLE FILE` greps, fd 3 is the
console (`echo "HOST ..." >&3`; a long test can stream its progress
there, os-test does). Keep the output of a passing test to itself:
failures show the last 40 lines.

A test that needs a peer outside the guest (a client connecting in) gets
one from the port's own `host.sh` (`PORT_HOST=host.sh`): the guest side
prints `HOST <port> <args>` to fd 3, the launcher runs
`bash <port dir>/host.sh <args>` on the host, and the guest side waits for
the effect (a file the session touched, a reply on its socket) with a
bound. The script's messages go to its stderr; its exit status is logged,
the guest test decides. A test boot's QEMU monitor is a unix socket the
scripts find in `$MYOS_QEMU_MONITOR`: `user/c/host.sh sendkey KEYS...` types
on the guest's keyboard through it (`/dev/console/kbd`), the keys in order
(several `HOST` requests run in parallel, so one per key would not keep it),
and `user/c/host.sh mouse` moves and clicks the test boots' `usb-mouse`
(`/dev/console/mouse`), `mouse ps2` and `mouse virtio` QEMU's PS/2 mouse
(x86) and virtio mouse (the `virt` boards' test boots carry one) through
`mouse_set`, and back.

## On the host

`src/boot_test.rs` starts QEMU with the serial console on its stdio
(`src/main.rs` sets up the machine (x86: `q35` with Intel's IOMMU, or
AMD's under `MYOS_X86_IOMMU=amd`, aarch64: `virt` with an SMMUv3, riscv64: `virt` with the RISC-V IOMMU
where QEMU is 9.2 or later, `docs/pci-acpi-smp.md`), the disks, the network with the port
forwards, the simulated Wi-Fi radio and its access points
(`src/wifi_sim.rs`, `docs/wifi.md`), unless `net` is skipped the package
mirror and the test web server, and, with the
Linux layer and unless `alpine-disk` is skipped, the Alpine disk:
`linux-compat/alpine-disks.sh` builds it once into
`target/alpine-<arch>.img`, the roots of rustc, Python and an X client
from `get-alpine -n` on the host, attached as `/dev/nvme2n1/data` with its
writes kept in a QEMU snapshot), waits for `login: ` on the serial port
(`ttyS0`'s getty, `docs/tty.md`; a kernel status line that lands after a
prompt is skipped), types `root`, an empty password, `COLUMNS=1000` and the command
(each byte once its echo is back, so an AP's lagging echo never garbles the line; the
`COLUMNS` line keeps the shell's line editor from scrolling a long command sideways, as it
would on the `virt` boards' 100-column console), then watches:

- the `TEST`, `HOST` (the named port's `host.sh`) and `TESTS DONE` lines;
- the kernel's crash reports (`exception:`, `user panic`, a user fault)
  end the run at once;
- a **stall watchdog**: the console must print something within 3 minutes
  (a quick run) or 10 minutes, and the whole run has its budget. A stall
  ends with every CPU's registers from the QEMU monitor (`info registers
  -a`): where the guest spins or halts, by `addr2line` on the kernel;
- the kernel's own **boot markers** (`[ OK ] heap`, `[ OK ] dma pages`, `[ OK ] scheduler`,
  the drivers, the VFS checks of `/bin/custom/ok`), which init prints
  before the login prompt: required whatever the tests say. `ok` runs on
  every boot, so it writes to a disk (`mkfs.ext2` on `/dev/nvme0n1/data`) and
  expects the FAT mount only once it found the launcher's FAT volume
  (`/msg` reading `fat-msg`); in another VM it leaves the disks alone;
- after the tests, `poweroff` typed at the prompt: QEMU must exit by
  itself, the console naming the method (`power off via ...`,
  `docs/power.md`);
- then `e2fsck -fn` on the scratch disk (`target/scratch.img`,
  the guest's `/dev/nvme1n1/data`) when the ext2 tests left a filesystem on it,
  and `fsck.fat -n` on the FAT test disk (`target/fat.img`, the guest's
  `/dev/vda/data`, which `fat_rw` writes to); each skipped without its tool
  (e2fsprogs, dosfstools).

It exits 0 only when every test passed, every marker was seen and the
machine powered off, and prints a one-line summary with the failed tests'
names. The full serial output is in the log either way.

The boot disk is a copy of the image (`target/boot-test-<name>.img`), so a
test writing to it leaves the build's image as built; myos sees it on every
arch (a virtio disk; on x86 a third legacy virtio-blk, `vdc`, after the
test disks, which keep their names: not NVMe, whose 64-bit BAR SeaBIOS
cannot reach with 4 GiB of RAM). After a full list that passed, the launcher
boots the copy again and types `sh /lib/myos-tests/run.sh reboot`, which
checks only that the system came up from slot `b` at the release the
upgrade wrote, and that the app the list installed in `/data/apps` (on the
boot disk's data partition) is still there and runs; then it boots the scratch disk `get-myos --install --local`
made (the list's last test, `t_last` in `run.sh`: mkfs.ext2's formats the
whole scratch disk; kept as `target/installed-<name>.img`) as the boot disk, by BIOS on the
bios job and UEFI on the others, and `run.sh installed` checks it came up
from its slot `a`.

## In CI

`ci-runtime.yml` boots nothing fixed: its `plan` job runs
`scripts/ci-select.py`, which reads every test's tags (the `t -t` lines,
the ports' names from the descriptors) and the paths the change touches,
and writes the boot job's matrix. A path selects tags:

| changed | selects |
|---|---|
| a port's or a package's directory | its tests (its name), those of the ports built on it or needing it (`PORT_DEPS`, `PORT_RDEPS`) and what it implements (netd: `netfs`; mkfs.ext2: `ext2`; get-myos: `install`, `pkg`) |
| `modules/<name>` | the module's tag (console: `console`, `tty`; the disk drivers: `blk`, sdmmc also `sdmmc` and `sdio`; the USB modules: `usb`, usb_audio and uhci also `sound`; virtio_sound: `sound`; virtio_net: `virtio_net`, `netfs`; pl061: `irq`); `modules/abi`, `acpi` and `pci_enum`: everything |
| `linux-compat/` | `linux` |
| `kernel/src/arch/<arch>` | everything on that arch's machines, variants too, the long groups included; the tags (`smoke` at least) on the other arches' primary machines |
| `kernel/src/iommu/`, `kernel/src/limine_boot.rs`, `src/`, `boot/`, `.github/`, `scripts/ci-*` | everything on every machine: what differs between an arch's machines |
| `user/get-myos`, `linux-compat/`, `packages/os-test` | their tags, with the long groups on aarch64 and riscv64 too (below) |
| `kernel/`, `toolchain/`, `build.rs`, `scripts/`, `etc/`, `user/init`, `user/tests`, anything else | everything on the primary machines, `smoke` on the variants |
| documentation (`*.md`, `docs/`, `licenses/`) | nothing: no boot |

`smoke` is always in. A push to master, the daily run, a dispatch with
`full_boot` and a pull request labelled `ci:full` run everything on every
machine. The image carries the Linux layer (`linux_compat`) when a
`linux` test runs.

The machines are bios, uefi, bios with AMD's IOMMU (`bios-amd`: its light
boot only, the long groups exercise nothing the unit changes), aarch64,
aarch64 with GICv3, riscv64 (QEMU's AIA: the APLIC and the IMSICs, the
PCI devices' MSIs) and riscv64 with the PLIC (`riscv64-plic`,
`MYOS_RISCV_INTC=plic`: INTx only; its light boot only, as `bios-amd`)
(the Raspberry Pi 3, below, runs its own).
Each arch has a primary machine (uefi, aarch64-gicv3, riscv64), the one a
pull request's selection boots on; the others are variants of it, in the
firmware, the IOMMU or the interrupt controller, and boot the smoke set
for a change that selects everything, nothing for one that selects tags:
what sets them apart is exercised by the machine-specific paths of the
table (everything on every machine) and by master's runs.

On aarch64 and riscv64, which run under TCG, the long groups take two to
three times what the light boot does (below: 16 minutes against 9 on
riscv64), so a pull request boots the light tests there alone, whatever
it selects, and the groups on x86, under KVM, where they take 3 to 6
minutes. The exceptions, which boot the groups on aarch64 and riscv64 in a
pull request too, are the paths whose changes the groups have caught
breaking one arch before: `user/get-myos` (riscv64's lseek, brk heap and
ESP broke the install and the upgrade), `linux-compat/` (musl's vfork on
aarch64 and riscv64) and `packages/os-test` (its semantics have per-arch
issues open), and `kernel/src/arch/<arch>`, which boots everything on its
arch. The runs of everything (master, daily, `ci:full`) boot the groups
on every machine, so a port's or the kernel's arch-specific breakage in
them is found within the hour of merging. A pull request that touches the
kernel thus boots 10 times instead of 16 (its longest boot the riscv64
light one), one that touches a package 5 times, four of them smoke boots.

A machine's selection runs in parallel boots: the long tests' groups
(`ostest` and `alpine`: the curated os-test list and the apk installs,
whose tests build on each other; `rust`: the rust package's, which sets
the toolchain up in the guest, a download of some 230 MB, and builds with
it under the Linux layer, 11 minutes under TCG; `pkg`: the packages' install from the
mirror, the packages' and get-myos's tests on it and the `install` tests
of the boot disk, a boot of 13 minutes on riscv64 that the light boot
no longer carries) each in a boot of their own, whole, and one light
boot with everything else (9 minutes on riscv64), so a full run is five
boots per machine instead of one.
Each boot
gets a budget from what it runs (riscv64 half as much again); a
selection without `net` tests skips the mirror, one without `alpine-disk`
tests the Alpine disk, which a boot that needs it restores from the
Actions cache (keyed by `linux-compat/alpine-disks.sh --hash`: the scripts
that make it; bump `FIXTURE` there for a fresh one, Alpine's
latest-stable moves) or makes on a miss.

The Raspberry Pi 3 (QEMU `raspi3b`, `myos rpi3 test`) boots once
whenever an aarch64 machine does, with a fixed selection: the `smoke`
tests it can run, and the USB tests of its devices (`usb_disk`, `mouse`,
`usb_keyboard`, `usbdev`: QEMU's hub on its one root port, the stick, the
mouse and the serial adapter behind it, the keyboard the test plugs in;
the host scripts take the bus
and the hub's port from `MYOS_USB_BUS` and `MYOS_USB_PORT`, which the
launcher sets per board). It has no disk but the SD card it boots from
(`sdmmc` checks it, on the sdhost), no network or PCI, so the tests that
need the scratch disk (`partitions`, `ext2_disk`, `fstab_mount`,
`fileio`), the network (`boot_rc`: the rc.local's dropbear; `heap_mini`:
its ping and socket stages), another pointer (`ps2_mouse`,
`virtio_mouse`, `xev_pointer`) or a root port of their own
(`usb_hotplug`, `usb_hotplug_busy_reuse`, `usb_hub_replug`) are skipped,
and its boot checks the markers it can print (`boot_test::Board::Rpi3`). The build job builds its boot files
(`boot/rpi3/build.sh`, U-Boot cached by its pin) and packs them with the
rest.

The x86 boots (bios, uefi, bios-amd) run under KVM: the plan says so
(`accel`), the boot job's container takes the runner's `/dev/kvm` (GitHub's
Linux runners have it), and the launcher boots with `-accel kvm` as above.
The other arches run under TCG, on x86 runners.

The plan is in the run's summary;
`scripts/ci-select.py --changed PATH...` prints it locally for any set of
paths, `--all` the full one.

## Adding a test

- A port's or a program's behaviour: a function and a `t -t tags name
  function` line in the `test.sh` next to its `port.env` (`PORT_TEST`; a
  `host.sh` with `PORT_HOST` when the test needs a peer on the host).
  Programs the tests need (smokes) go in `user/c` or as a `user` crate,
  with their test.
- The kernel's own behaviour (a limit, a module without a user-side
  program): `user/tests/kernel.sh`, tagged with the module or area.
- A POSIX conformance case: the curated os-test lists
  (`packages/os-test/overlay/misc/*.tests`), which must pass in full.

Tag what the test needs (`net`, `pkg`, `heavy`, `install`) and what it
covers (the module's or area's name): the needs decide which runs it can
be in, the coverage which changes must run it. Test on every arch the
change could affect (arch-specific code: all three); a quick run per arch
is usually enough locally, CI does the rest. The `net` tests need the
network: in a sandbox behind a TLS-intercepting proxy, append its CA to
`target/cacert.pem` for local runs only and restore it afterwards.
