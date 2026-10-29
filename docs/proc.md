# Processes in /proc

`/proc` shows every process and its threads as text files, the way `ps` or
`top` reads them. `ps` (below) lists them, and the `bottom` package's `btm`
is an interactive `top`. `/proc` also holds the memory counters and a few
files about the hardware and the boot. The process files are read-only for
now; control files are issue #291.

## Files

Processes, CPUs and memory:

```
/proc/<pid>/status                  the process, one line
/proc/<pid>/task/                   its threads (their tids), the leader first
/proc/<pid>/task/<tid>/status       a thread, one line
/proc/self/exe                      symlink to the program the process runs: the real path of the file its exec loaded (a script's interpreter)
/proc/self/limits                   its resource limits, Linux's table: the main thread's stack and the fd table have one, nothing else ("unlimited")
/proc/self/stack                    its main thread's stack: its lowest address and its end, in hex (`lo-end`)
/proc/cpu                           the uptime and each CPU's idle time
/proc/meminfo                       RAM: total, free, available; the allocator's counters
/proc/boot/kernel                   the kernel file this boot came from, as Limine loaded it
/proc/boot/initramfs                the initramfs this boot came from (get-myos --install --local)
/proc/boot/tests                    the tests module, on the build's disks (docs/testing.md)
/proc/boot/partuuid                 the GPT partition it booted from, its unique GUID (mount -a: /boot)
```

The hardware and the system:

| File | What |
|------|------|
| `/proc/mounts` | the mount table (`docs/fs.md`) |
| `/proc/cpuinfo` | the online CPUs, their hardware ids, schedule counts |
| `/proc/platform` | the board description and where each component came from (`docs/platform.md`) |
| `/proc/firmware` | on a Raspberry Pi, what the VideoCore firmware says of the board: the firmware's and board's revisions, the clocks, the USB controller's power (`docs/raspberry-pi.md`) |
| `/proc/usb` | the controllers (ports, interrupt, event counts) and the USB devices: id, parent hub and port, speed, vendor:product, each interface's class triple and the driver that took it (`docs/usb.md`) |
| `/proc/pci` | the full BDF list from `pci_enum` (below) |
| `/proc/acpi/info`, `tables`, `s5` | from the `acpi` module; honest stubs when there is no RSDP |

`docs/fs.md` lists the other generated files (`/proc/partitions`,
`/proc/cmdline`, the host name, the security files).

### /proc/pci

`/proc/pci` lists every PCI function `pci_enum` found: hex IDs, class and
subclass names, and names from a small table of QEMU and virt devices.

Writing `rescan` to it re-enumerates the bus and refreshes the node, so
gone devices disappear. The kernel then calls every module's
`module_rescan`, and the block drivers (virtio-blk, virtio-scsi, NVMe)
bring up the disks that appeared since boot, leaving the known ones alone.
virtio-net probes once, at load: netd binds the one `/dev/net0`. There is
no ACPI or QEMU hotplug interrupt yet, so a hot-added disk shows up after a
rescan.

## How the files are read

The text files are made when they are read: each shows the moment of the
read. An open file reads one such text, the one a read at offset 0 took
(`MountOps::text`, `kernel/src/task/fd.rs`). So a reader that takes a file
in several `read`s (a shell's `while read`) gets whole lines of one version
of it. A read at offset 0 (a new open, `lseek` back to 0) takes a fresh
one. That is Linux's `seq_file` for a whole-file read.

## Processes and threads

`ls /proc` lists the processes. A thread is a task slot, and its tid is the
slot (`docs/threads.md`), so pids and tids share one number space. Any tid
also resolves as `/proc/<tid>`, which is then its process's directory (as
on Linux), but only process leaders are listed.

The idle tasks are process 0. The boot CPU's idle task is its leader, and
every other CPU's idle task is one of its threads. Kernel threads
(`spawn_named`, a module's `thread_spawn`) are processes of their own,
marked `kernel`.

## Status lines

A status line is one line of fields separated by single spaces, in the
order below. A name is one word: a byte that is not printable ASCII, a
space included, shows as `_`, and `-` stands for none. Times are
milliseconds. CPU time counts since the task started; other times are on
the monotonic clock (since boot).

`/proc/<pid>/status`:

| Field | What |
|-------|------|
| pid | |
| name | the program it last exec'd (its basename), its parent's for a forked child that has not exec'd yet, or a kernel thread's name |
| state | `running`, `ready` or `blocked`: the busiest of its threads (a leader that only waits for its other threads to end does not count); `zombie` once it has exited |
| ppid | 0 for none |
| pgid, sid | process group, session |
| threads | |
| size | virtual size in KiB: image, stack, heap up to the break, mmap regions |
| cpu | its threads' CPU time, those that ended included |
| children | the CPU time of the children it has waited for, theirs included |
| start | when it started |
| kind | `user` or `kernel` |
| faults | its threads' page faults, those that ended included (`/proc/meminfo`'s `Faults` is every process's together) |

```sh
read pid name state ppid pgid sid threads size cpu children start kind faults < /proc/$$/status
```

`/proc/<pid>/task/<tid>/status`:

| Field | What |
|-------|------|
| tid, pid | |
| name | as above, per thread (a new thread is named after its creator) |
| state | `running`, `ready`, `blocked` or `zombie` |
| cpu | its home CPU (an idle CPU may take it, `docs/scheduler.md`), `-` when it may run anywhere |
| wait | what it is blocked on (`block_until`'s key, hex), `0x0` when it is not blocked |
| time | its CPU time |
| start | when it started |
| pending, blocked | its pending and blocked signals (hex bit masks, bit N = signal N) |

`/proc/cpu`:

```
uptime 81234
cpu0 idle 60311
cpu1 idle 71552
```

A CPU's idle time is the time it spent halted (`hlt` / `wfi`). The busy
share over an interval is `1 - Δidle / Δuptime`.

## ps

`ps` (`/bin/custom/ps`, a shell script: `user/ps/ps`) prints a row per
process from these files, by pid: pid, parent, state, threads, virtual
size, CPU time and name.

- Without options it lists the user processes.
- `-e` adds the kernel's: the idle tasks and the kernel threads.
- `-L` prints a row per thread instead, with its home CPU and CPU time.

A process that exits while `ps` reads it is left out.

```
$ ps
  PID  PPID STATE   THR  SIZE(K)     TIME NAME
    1    11 blocked   1     3004     0:00 netd
    2    11 blocked   1     1468     0:00 sh
   11     0 blocked   1     1168     0:00 init
```

## Memory

`/proc/meminfo` is the frame allocator's counters, as `Name: value` lines.
A system monitor reads these three, in KiB:

| Line | What |
|------|------|
| `MemTotalKiB` | all usable RAM |
| `MemFreeKiB` | the part nothing holds |
| `MemAvailableKiB` | that and the block and page caches, which give their memory back when it runs out |

The other lines:

| Line | What |
|------|------|
| `FramesAlloc`, `FramesFree` | frames allocated and freed since boot |
| `FramesLive`, `LiveKiB` | the live frames |
| `Site*` | allocations per kernel call site, for leak hunting |
| `Faults` | the page faults handled, every process's (a process's own are in its status line) |
| `FaultAround` | the pages the fault-around mapped besides the faulting ones (`docs/linux-compat.md`) |
| `Forks`, `CowCopies` | the forks, and the pages a store copied after one: a fork shares the parent's pages copy-on-write (`docs/fs.md`) |
| `Syscalls` | the system calls made |
| `TlbFlushes`, `TlbPageFlushes`, `TlbShootdowns` | the user TLB flushes of a whole address space and of one page, and the shootdowns sent to other CPUs |
| `BlockCacheKiB`, `PageCacheKiB` | the caches' sizes |

## How the time is counted

`task/acct.rs` keeps the counters, plain atomics per task slot and per CPU.

- A CPU charges the time since its last charge to its current task on
  every `schedule` (a tick, a switch) and before it halts. The halt itself
  is idle time, charged to nobody.
- A task alone on its CPU is charged this way at each tick. A task halting
  in `block_until` stays the CPU's current task, and is charged only up to
  the halt.
- A reading charges the reader first, so a task's own CPU time (from
  `clock_gettime` or `getrusage`) is current. It lags another running task
  by up to a tick (10 ms).
- An ending thread hands its time to its process, and `wait` adds the
  reaped child's time (its own children's included) to the parent's
  `children`.

## Reading it from a tool

Each file is made under one scheduler lock, so a line is consistent in
itself. Nothing is consistent across files.

- A process can exit between listing `/proc` and opening its `status`.
  `ENOENT` there is normal, and a reader drops the process.
- A pid is a task slot, and a new process takes the lowest free one, so a
  pid is reused within seconds. A tool that compares two samples keys a
  process on its pid and its start time.

Anyone can read every process's status lines. What else of another
process one may see (its fds, namespace, security context) is not decided
yet, so only `/proc/self` shows those.
