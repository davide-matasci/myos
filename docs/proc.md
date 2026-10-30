# Processes in /proc

`/proc` shows every process and its threads as text files, the way `ps` or
`top` reads them. It also holds the memory counters and a few files about
the hardware and the boot. `ps` (below) lists the processes, and the
`bottom` package's `btm` is an interactive `top`. The process files are
read-only for now; control files are issue #291. The code is
`kernel/src/fs/procfs.rs` and `kernel/src/task/info.rs`.

## Files

```
/proc/<pid>/status                  the process, one line
/proc/<pid>/task/                   its threads (their tids), the leader first
/proc/<pid>/task/<tid>/status       a thread, one line
/proc/self/exe                      symlink to the file its exec loaded (a script's interpreter)
/proc/self/limits                   Linux's table: the main thread's stack and the fd table have a limit, nothing else
/proc/self/stack                    its main thread's stack, `lo-end` in hex
/proc/cpu                           the uptime and each CPU's idle time
/proc/meminfo                       RAM and the allocator's counters
/proc/boot/kernel, initramfs        the files this boot came from (get-myos --install --local)
/proc/boot/tests                    the tests module, on the build's disks (docs/testing.md)
/proc/boot/partuuid                 the GPT partition it booted from (mount -a: /boot)
```

The hardware:

| File | What |
|------|------|
| `/proc/cpuinfo` | the online CPUs, their hardware ids, schedule counts |
| `/proc/platform` | the board description and where each part came from (`docs/platform.md`) |
| `/proc/firmware` | on a Raspberry Pi, what the VideoCore firmware reports (`docs/raspberry-pi.md`) |
| `/proc/usb` | the USB controllers and devices, and the driver that took each interface (`docs/usb.md`) |
| `/proc/pci` | every PCI function `pci_enum` found; writing `rescan` re-enumerates the bus (`docs/pci.md`, `docs/modules.md`) |
| `/proc/acpi/info`, `tables`, `s5` | from the `acpi` module; stubs when there is no RSDP |

`docs/fs.md` lists the other files (`/proc/mounts`, `/proc/partitions`,
`/proc/cmdline`, the host name, the security files).

A file's text is made when it is read at offset 0, and an open file keeps
reading that text (`MountOps::text`, `kernel/src/task/fd.rs`). So a reader
that takes a file in several `read`s gets whole lines of one version, as
with Linux's `seq_file`.

## Processes and threads

`ls /proc` lists the processes. A tid is a task slot (`docs/threads.md`),
so pids and tids share one number space. Any tid resolves as `/proc/<tid>`,
its process's directory, but only process leaders are listed.

The idle tasks are process 0: the boot CPU's idle task leads it, and the
other CPUs' are its threads. Kernel threads (`spawn_named`, a module's
`thread_spawn`) are processes of their own, marked `kernel`.

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
| state | `running`, `ready` or `blocked`: the busiest of its threads (a leader that only waits for its threads to end does not count); `zombie` once it has exited |
| ppid | 0 for none |
| pgid, sid | process group, session |
| threads | |
| size | virtual size in KiB: image, stack, heap up to the break, mmap regions |
| cpu | its threads' CPU time, those that ended included |
| children | the CPU time of the children it has waited for, theirs included |
| start | when it started |
| kind | `user` or `kernel` |
| faults | its threads' page faults, those that ended included |

```sh
read pid name state ppid pgid sid threads size cpu children start kind faults < /proc/$$/status
```

`/proc/<pid>/task/<tid>/status`:

| Field | What |
|-------|------|
| tid, pid | |
| name | as above, per thread (a new thread is named after its creator) |
| state | `running`, `ready`, `blocked` or `zombie` |
| cpu | its home CPU (`docs/scheduler.md`), `-` when it may run anywhere |
| wait | what it is blocked on (`block_until`'s key, hex), `0x0` when it is not blocked |
| time | its CPU time |
| start | when it started |
| pending, blocked | its pending and blocked signals (hex bit masks, bit N = signal N) |
| ignored, caught | its process's dispositions: the signals it ignores, the ones it catches (hex bit masks; the rest are at their default) |

`/proc/cpu`:

```
uptime 81234
cpu0 idle 60311
cpu1 idle 71552
```

A CPU's idle time is the time it spent halted (`hlt` / `wfi`). The busy
share over an interval is `1 - Δidle / Δuptime`.

CPU time is charged to the running task at every `schedule` and before a
CPU halts (`kernel/src/task/acct.rs`). A task reading its own time is
charged first, so it is current; another running task's lags by up to a
tick (10 ms). An ending thread's time goes to its process, and `wait` adds
a reaped child's time to the parent's `children`.

## ps

`ps` (`user/ps/ps`, a shell script) prints a row per process: pid, parent,
state, threads, virtual size, CPU time and name.

- Without options it lists the user processes.
- `-e` adds the kernel's: the idle tasks and the kernel threads.
- `-L` prints a row per thread, with its home CPU and CPU time.

```
$ ps
  PID  PPID STATE   THR  SIZE(K)     TIME NAME
    1    11 blocked   1     3004     0:00 netd
    2    11 blocked   1     1468     0:00 sh
   11     0 blocked   1     1168     0:00 init
```

## Memory

`/proc/meminfo` is the frame allocator's counters, as `Name: value` lines.
A system monitor reads three, in KiB:

| Line | What |
|------|------|
| `MemTotalKiB` | all usable RAM |
| `MemFreeKiB` | the part nothing holds |
| `MemAvailableKiB` | that and the block and page caches, which give their memory back when it runs out |

The others are for debugging and tests: frames allocated, freed and live
(`FramesAlloc`, `FramesFree`, `FramesLive`, `LiveKiB`), allocations per
kernel call site (`Site*`, for leak hunting), every process's page faults
(`Faults`) and the extra pages fault-around mapped (`FaultAround`,
`docs/linux-compat.md`), forks and copy-on-write copies (`Forks`,
`CowCopies`, `docs/fs.md`), `Syscalls`, the TLB flushes and shootdowns
(`TlbFlushes`, `TlbPageFlushes`, `TlbShootdowns`), and the caches' sizes
(`BlockCacheKiB`, `PageCacheKiB`).

## Reading it from a tool

Each file is made under one scheduler lock, so a line is consistent in
itself. Nothing is consistent across files.

- A process can exit between listing `/proc` and opening its `status`.
  `ENOENT` there is normal: drop the process.
- A new process takes the lowest free pid, so pids are reused within
  seconds. A tool comparing two samples keys a process on its pid and
  start time.

Anyone can read every process's status lines. What else of another process
one may see (its fds, namespace, security context) is not decided yet, so
only `/proc/self` shows those.
