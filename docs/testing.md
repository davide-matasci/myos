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
| tests | the shell, exec, the basic programs, ext2 on the scratch disk, the heavy smoke (`heap mini`), the Linux module, the C smokes, DNS, listen/accept, the tty | the same with `heap` (git's porcelain), plus: every package of the build installed from the host's mirror, HTTPS with the kernel's client and curl, two concurrent SSH sessions into dropbear, the Linux layer's Alpine packages (jq, Python), the curated os-test list, the packages' own tests |

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
HOST tcp-ping 2323
TESTS DONE 17/18
```

- `TEST <name> PASS|FAIL`: one per test.
- `HOST <what> <args>`: the test needs the host: `tcp-ping PORT` connects
  to the guest's listener through QEMU's port forward and plays ping/pong;
  `ssh PORT` opens two SSH sessions at once (the dropbear test key), each
  of which touches `/tmp/ssh-ok-a` / `-b` for the guest test to find.
- `TESTS DONE <passed>/<total>` ends the run.

The sections run in this order: `shell.sh` (the shell and the programs
every boot has, the heavy `heap` smoke), `linux.sh` (the Linux layer: its
smokes when the image was built with the feature, `insmod` otherwise),
the ports' tests (`ports/*.sh`, see below), `net.sh` (DNS, HTTPS,
listen/accept), the tty. In the full mode the very first test installs
every package the host's mirror has.

The tty test (`user/c/tty_smoke.c`) drives an interactive shell on a pty
the way a person types at the console: history recall and in-line editing
with the arrow keys, backspace, `^C` on a foreground pipeline with the
shell surviving.

### A port's own tests

A port ships its test with `PORT_TEST=test.sh` in its `port.env`
(`docs/ports.md`): the file lands at `/lib/myos-tests/ports/<name>.sh` in
the image, or in the package, so moving a port between `ports/` and
`packages/` moves its test too. The runner sources every file of that
directory in name order, after the core sections. A test script calls `t`:

```sh
# packages/make/test.sh
make_version() {
	grep -q /bin/custom/make /proc/mounts && make --version | grep -q "GNU Make"
}
t make make_version
```

`$MODE` is `mini` or `full` (a script that needs the network starts with
`[ "$MODE" = full ] || return 0`), `$OUT` is the output directory,
`contains NEEDLE FILE` greps, fd 3 is the console (`echo "HOST ..." >&3`;
a long test can stream its progress there, os-test does). Keep the
output of a passing test to itself: failures show the last 40 lines.

## On the host

`src/boot_test.rs` starts QEMU with the serial console on its stdio
(`src/main.rs` sets up the machine, the disks, the network with the port
forwards, and the package mirror in the full mode), waits for `login: `,
types `root`, an empty password and the command (each byte once its echo
is back, so an AP's lagging echo never garbles the line), then watches:

- the `TEST`, `HOST` and `TESTS DONE` lines;
- the kernel's crash reports (`exception:`, `user panic`, a user fault)
  end the run at once;
- a **stall watchdog**: the console must print something within 3 minutes
  (mini) or 10 minutes (full), and the whole run has its budget;
- the kernel's own **boot markers** (`[ OK ] heap`, `[ OK ] scheduler`,
  the drivers, the VFS checks of `/bin/custom/ok`), which init prints
  before the login prompt: required whatever the tests say;
- after the boot, `e2fsck -fn` on the scratch disk (`target/scratch.img`,
  the guest's `/dev/nvme1n1`) when the ext2 tests left a filesystem on it;
  skipped without e2fsprogs.

It exits 0 only when every test passed and every marker was seen, and
prints a one-line summary with the failed tests' names. The full serial
output is in the log either way.

## Adding a test

- Something every boot must do: a function in the matching section of
  `user/tests/` and a `t name function` line. Programs the tests need
  (smokes) go in `user/c` or as a `user` crate.
- A port's behaviour: `test.sh` next to its `port.env`, with `PORT_TEST`.
- A POSIX conformance case: the curated os-test lists
  (`packages/os-test/overlay/misc/*.tests`), which must pass in full.

Test on every arch the change could affect (arch-specific code: all three);
a mini run per arch is usually enough locally, CI does the rest. The full
mode needs the network: in a sandbox behind a TLS-intercepting proxy, append
its CA to `target/cacert.pem` for local runs only and restore it afterwards.
