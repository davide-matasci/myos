# Boot tests

The real test of myos is booting it. The launcher boots an image headless
in QEMU, logs in and runs a test list **in the guest**; the host only
watches the serial console for the result.

```sh
cargo run -- test-mini               # x86_64 BIOS, the quick list (what a pull request runs)
cargo run -- aarch64 test-mini       # also uefi, riscv64
cargo run -- test-full               # the full list (needs the network)
scripts/local-ci.sh [bios|uefi|aarch64|riscv64] [mini|full]   # the same, with the OOM/TCG settings for a loaded host
```

A boot without `test-mini`/`test-full` is a normal boot to the login
prompt (what the ISO is for).

## The two lists

| | `test-mini` | `test-full` |
|---|---|---|
| CI | every pull request, on bios, uefi, aarch64 and riscv64 | the daily scheduled run and `workflow_dispatch` with `full_boot` |
| budget | 4 minutes (5 with the Linux layer) | 50 minutes |
| network | QEMU's user network only (DNS and the listen test go through it) | the host's: HTTPS, the Alpine mirror, this build's packages |
| tests | the shell, exec, the basic programs, GPT partitions and ext2 on the scratch disk, FAT read-write, the heavy smoke (`heap mini`), the Linux module, the C smokes, DNS, listen/accept, the tty | the same with `heap` (git's porcelain and uutils, apps), plus: every package of the build installed from the host's mirror as an app, HTTPS with the kernel's client and curl, two concurrent SSH sessions into dropbear and a login on a pty, the Linux layer's Alpine packages (jq, Python, an X client on the native X server, and rustc from a disk the host prepares), the curated os-test list, the packages' own tests, `get-myos --upgrade` of the boot disk (Limine made stale first) and `--install` on the scratch disk, then a **second boot** of the upgraded disk, which must come up from slot `b`, and a **third** of the disk `--install --local` made (by BIOS on the bios job) (`docs/install.md`) |

## In the guest

The runner is `/lib/myos-tests/run.sh` (from `user/tests`); after login
the launcher types

```sh
sh /lib/myos-tests/run.sh mini|full
```

which anyone can type at the prompt too. One test is a command, usually a
shell function; its exit status decides. Its output goes to
`/tmp/myos-tests/<name>.out` and is printed only when it fails, so the
console shows one line per test:

```
TEST shell_echo PASS
TEST heap FAIL (exit 1)
    smoke start
    ...
HOST tests tcp-ping 2323
TESTS DONE 17/18
```

- `TEST <name> PASS|FAIL`: one per test.
- `HOST <port> <args>`: the test needs the host. The launcher runs that
  port's host-side script (`PORT_HOST=host.sh` in its `port.env`, from the
  checkout) with the arguments, in the background; the guest test then
  waits for what the script does. `c-smokes tcp-ping PORT` (`user/c/host.sh`)
  connects to the guest's listener through QEMU's port forward and plays
  ping/pong; `tests usb-plug|usb-unplug` (`user/tests/host.sh`) plugs a
  second USB stick into the guest through the QEMU monitor and pulls it out
  (`docs/usb.md`); `dropbear PORT` (`ports/dropbear/host.sh`) opens two SSH
  sessions at once with the test key, each of which touches
  `/tmp/ssh-ok-a` / `-b` for the guest test to find, then a login on a pty
  (`ssh -tt`) that writes its terminal to `/tmp/ssh-tty`. A port that needs a
  peer on the host ships its own `host.sh`; the launcher knows no test.
- `TESTS DONE <passed>/<total>` ends the run.

Every test belongs to the port of what it tests and ships with it
(`test.sh`, see below): the shell's are oksh's, `heap`'s are `user/heap`'s,
the ext2 ones are `mkfs.ext2`'s, DNS and HTTPS are the `dns` and `http`
programs', listen/accept and the tty are the C smokes'. The one section of
`user/tests` itself, `kernel.sh`, holds what has no port directory: the
exec limits and the Linux layer (its smokes when the image was built with
the feature, `insmod` otherwise). The runner sources `kernel.sh`, then
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

`$MODE` is `mini` or `full` (a script that needs the network starts with
`[ "$MODE" = full ] || return 0`), `$OUT` is the output directory,
`contains NEEDLE FILE` greps, fd 3 is the console (`echo "HOST ..." >&3`;
a long test can stream its progress there, os-test does). Keep the
output of a passing test to itself: failures show the last 40 lines.

A test that needs a peer outside the guest (a client connecting in) gets
one from the port's own `host.sh` (`PORT_HOST=host.sh`): the guest side
prints `HOST <port> <args>` to fd 3, the launcher runs
`bash <port dir>/host.sh <args>` on the host, and the guest side waits for
the effect (a file the session touched, a reply on its socket) with a
bound. The script's messages go to its stderr; its exit status is logged,
the guest test decides. A test boot's QEMU monitor is a unix socket the
scripts find in `$MYOS_QEMU_MONITOR`: `user/c/host.sh sendkey KEYS...` types
on the guest's keyboard through it (`/dev/console/kbd`), the keys in order
(several `HOST` requests run in parallel, so one per key would not keep it).

## On the host

`src/boot_test.rs` starts QEMU with the serial console on its stdio
(`src/main.rs` sets up the machine, the disks, the network with the port
forwards, and in the full mode the package mirror and, with the Linux
layer, the Alpine Rust disk: `linux-compat/alpine-disk.sh` builds it once
into `target/alpine-rust-<arch>.img`, attached as `/dev/nvme2n1/data` with its
writes kept in a QEMU snapshot), waits for `login: `,
types `root`, an empty password and the command (each byte once its echo
is back, so an AP's lagging echo never garbles the line), then watches:

- the `TEST`, `HOST` (the named port's `host.sh`) and `TESTS DONE` lines;
- the kernel's crash reports (`exception:`, `user panic`, a user fault)
  end the run at once;
- a **stall watchdog**: the console must print something within 3 minutes
  (mini) or 10 minutes (full), and the whole run has its budget. A stall
  ends with every CPU's registers from the QEMU monitor (`info registers
  -a`): where the guest spins or halts, by `addr2line` on the kernel;
- the kernel's own **boot markers** (`[ OK ] heap`, `[ OK ] scheduler`,
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

## Adding a test

- A port's or a program's behaviour: a function and a `t name function`
  line in the `test.sh` next to its `port.env` (`PORT_TEST`; a `host.sh`
  with `PORT_HOST` when the test needs a peer on the host). Programs the
  tests need (smokes) go in `user/c` or as a `user` crate, with their test.
- The kernel's own behaviour (a limit, a module without a user-side
  program): `user/tests/kernel.sh`.
- A POSIX conformance case: the curated os-test lists
  (`packages/os-test/overlay/misc/*.tests`), which must pass in full.

Test on every arch the change could affect (arch-specific code: all three);
a mini run per arch is usually enough locally, CI does the rest. The full
mode needs the network: in a sandbox behind a TLS-intercepting proxy, append
its CA to `target/cacert.pem` for local runs only and restore it afterwards.
