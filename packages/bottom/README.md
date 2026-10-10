# bottom for myos

[bottom](https://github.com/ClementTsang/bottom) (`btm`), a terminal system
monitor in Rust, built on the myos `std`: its CPU, memory and process
widgets, with the kernel's numbers from `/proc`.

```sh
get-myos bottom
run-myos bottom                 # q quits; dd on a process sends it a signal
```

## What it shows

- **CPU**: each CPU's busy share since the last refresh, from the idle
  times in `/proc/cpu`, and their average.
- **Memory**: RAM used, from `/proc/meminfo`'s `MemTotalKiB` and
  `MemAvailableKiB` (`docs/proc.md`). myos has no swap.
- **Processes**: every `/proc/<pid>/status`, with its name, parent, state
  (running, sleeping, zombie), CPU share (its CPU time over the time between
  two refreshes) and memory. myos keeps no resident size per process: the
  memory column is the virtual size (image, stack, heap, mappings). The
  kill dialog lists myos's signals (newlib's numbers).

Disks, networks, temperatures, batteries and GPUs are not reported (the
build has no default features, and sysinfo's myos backend leaves those
parts empty); neither is the load average. The configuration file is
`$HOME/.config/bottom/bottom.toml`.

## What it is built from

| Component | Pin | |
|-----------|-----|-|
| bottom | `versions.env` (crates.io, sha256) | MIT |
| its Rust dependencies | bottom's `Cargo.lock` | MIT / Apache-2.0 and similar permissive licenses; `option-ext` (used unmodified): MPL-2.0 (`THIRD_PARTY_NOTICES.md`) |

`fetch.sh` downloads bottom and the four crates myos patches (crossterm,
sysinfo, dirs-sys, parking_lot_core) at the versions bottom's `Cargo.lock`
pins; `build.sh` patches them and builds bottom against them
(`[patch.crates-io]`), with the patched `libc`, `rustix` and `errno` of the
other Rust ports (`packages/coreutils/prepare.sh`). The patches are under
their crates' licenses:

- `sysinfo.myos.patch`: a myos backend (`src/myos/`) reading `/proc/cpu`,
  `/proc/meminfo` and `/proc/<pid>/status`; the rest is sysinfo's
  `unknown` backend.
- `crossterm.myos.patch`: the unix terminal code for myos, and an event
  source that polls the tty (`poll`) and checks the window size every
  250 ms (myos sends no `SIGWINCH`).
- `parking_lot_core.myos.patch`: a thread parker on `std::thread::park`
  (the generic one spins).
- `dirs-sys.myos.patch`: the home directory from `$HOME`.
- `bottom.myos.patch`: myos in bottom's unix `cfg`s, its signal table and
  process states.

## The test

`test.sh` (full mode, after the install): `btm_smoke` runs `btm` on a pty
(120×40), answers its cursor position query, waits for the CPU, memory and
process widgets with `init` and `btm` in the list, then types `q` and
expects `btm` to exit 0. On a failure it prints the end of the screen and
each `btm` thread's `/proc` line.
