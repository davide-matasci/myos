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

## Selecting tests

Every test carries **tags**: what it needs, what it covers, and its own
name. `test` takes the tests with any of the `--tags` (all of them
without), minus those with any of the `--skip` tags; so `--tags dns` runs
that one test and `--skip heap` leaves that one out.
`test-mini` and `test-full` are the two lists of old: `test --skip
net,pkg,heavy,install,alpine-disk` and `test`.

| tag | the tests that |
|---|---|
| `net` | need the host's network: HTTPS, the package mirror the launcher serves, the Alpine mirror (without it a run stays on QEMU's user network: DNS and the listen test go through it) |
| `pkg` | need the packages of this build installed as apps (`packages`, the first test, installs them from the mirror; every package's own test has it) |
| `heavy` | take minutes under TCG: the curated os-test list, the Alpine apk installs (jq, netpbm), the heavy smoke with the apps |
| `alpine-disk` | mount the Alpine roots the host prepares (rustc, Python, an X client: `linux-compat/alpine-disks.sh`, fetched once into `target/alpine-<arch>.img`, attached as `/dev/nvme2n1/data`) |
| `install` | change the boot disk: `get-myos --upgrade` (Limine made stale first) and `--install` on the scratch disk; when they passed, the launcher boots the upgraded disk again, which must come up from slot `b`, and then the disk `--install --local` made, from its slot a (`docs/install.md`) |
| `smoke` | are the quick basics: the shell, exec, the basic programs, the filesystems on the scratch disk, `heap mini`; about two minutes |
| a module's name (`netfs`, `console`, `linux`, `ext2`, `fat`, `usb`, `blk`, `virtio_net`, ...) or an area's (`sec`, `tty`, `events`, `mm`, `sched`, `proc`, `fs`, `exec`, `x11`, `pci`) | cover it: what a change there must run |
| a port's name (`oksh`, `c-smokes`, `curl`, `os-test`, ...) | come from that port: its `test.sh` (every test of the file, set by the runner) |
| `alpine`, `python`, `x11`, `ostest` | the long tests by what they build, for a run of one of them alone |

A quick run (`heavy` and `net` skipped) has a budget of 4 minutes (5 with
the Linux layer; half as much again on riscv64, the slowest under TCG, as
in CI) and a stall limit of 3; any other 50 and 10
(`--budget MINUTES` sets the budget). The runner prints what it skipped
(`TESTS SKIPPED <n>`) after `TESTS DONE`.

## In the guest

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
  waits for what the script does. `c-smokes tcp-ping PORT` (`user/c/host.sh`)
  connects to the guest's listener through QEMU's port forward, plays
  ping/pong and then opens and closes forty connections without a word; `tests usb-plug|usb-unplug` (`user/tests/host.sh`) plugs a
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
the packages last. In the full mode the very first test installs every
package the host's mirror has, as apps in `/tmp/apps` (`MYOS_APPS`,
`docs/packages.md`), and the runner sources the packages' tests from
there (`<app>/lib/myos-tests/ports/`) with the image's, in the same name
order; they run their programs with `run-myos`.

The tty test (`user/c/tty_smoke.c`, in `user/c/test.sh`) drives an
interactive shell on a pty the way a person types at the console: history
recall and in-line editing with the arrow keys, backspace, `^C` on a
foreground pipeline with the shell surviving.

### A port's own tests

A port ships its test with `PORT_TEST=test.sh` in its `port.env`
(`docs/ports.md`): the file lands at `/lib/myos-tests/ports/<group>-<name>.sh`
in the image, or in the package's app (`0` for a core image port, `1` for
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
(`src/main.rs` sets up the machine (x86: `q35` with Intel's IOMMU,
aarch64: `virt` with an SMMUv3, `docs/pci-acpi-smp.md`), the disks, the network with the port
forwards, the simulated Wi-Fi radio and its access points
(`src/wifi_sim.rs`, `docs/wifi.md`), unless `net` is skipped the package
mirror and, with the
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
| `modules/<name>` | the module's tag (console: `console`, `tty`; the disk drivers: `blk`; the USB ones: `usb`; virtio_net: `virtio_net`, `netfs`); `modules/abi`, `acpi` and `pci_enum`: everything |
| `linux-compat/` | `linux` |
| `kernel/src/arch/<arch>` | everything on that arch's machines, `smoke` on the others |
| `kernel/`, `toolchain/`, `src/`, `build.rs`, `scripts/`, `.github/`, `etc/`, `user/init`, `user/tests`, anything else | everything |
| documentation (`*.md`, `docs/`, `licenses/`) | nothing: no boot |

`smoke` is always in. A push to master, the daily run, a dispatch with
`full_boot` and a pull request labelled `ci:full` run everything. The image
carries the Linux layer (`linux_compat`) when a `linux` test runs.

The selected tests run on every machine (bios, uefi, aarch64, aarch64 with
GICv3, riscv64) in parallel boots: the long tests' groups (`ostest`,
`alpine`: the curated os-test list, the apk installs) each in a boot of
their own, whole (their tests build on each other), and one boot with
everything else, so a full run is three boots of ten minutes per machine
instead of one of thirty. Each boot
gets a budget from what it runs (riscv64 half as much again); a
selection without `net` tests skips the mirror, one without `alpine-disk`
tests the Alpine disk, which a boot that needs it restores from the
Actions cache (keyed by `linux-compat/alpine-disks.sh --hash`: the scripts
that make it; bump `FIXTURE` there for a fresh one, Alpine's
latest-stable moves) or makes on a miss. The plan is in the run's summary;
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
