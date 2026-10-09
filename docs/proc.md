# Processes in /proc

`/proc` shows every process and its threads as text files, the way a `ps`
or a `top` reads them: `ps` (below) lists them, and the `bottom` package's
`btm` is an interactive `top`. It is read-only for now; control files are
issue #291.

```
/proc/<pid>/status                  the process, one line
/proc/<pid>/task/                   its threads (their tids), the leader first
/proc/<pid>/task/<tid>/status       a thread, one line
/proc/cpu                           the uptime and each CPU's idle time
/proc/meminfo                       RAM: total, free, available; the allocator's counters
/proc/boot/kernel                   the kernel file this boot came from, as Limine loaded it
/proc/boot/initramfs                the initramfs this boot came from (get-myos --install --local)
```

`ls /proc` lists the processes. A thread is a task slot and its tid is the
slot (`docs/threads.md`), so pids and tids share one number space: any tid
also resolves as `/proc/<tid>`, which is then its process's directory (as on
Linux), but only process leaders are listed.

The idle tasks are process 0: the boot CPU's is its leader, every other
CPU's idle task one of its threads. Kernel threads (`spawn_named`, a
module's `thread_spawn`) are processes of their own, marked `kernel`.

## Status lines

One line of fields separated by single spaces, in this order; a name is one
word (a byte that is not printable ASCII, a space included, shows as `_`;
`-` for none). Times are milliseconds: CPU time since the task started,
other times on the monotonic clock (since boot).

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

```sh
read pid name state ppid pgid sid threads size cpu children start kind < /proc/$$/status
```

`/proc/<pid>/task/<tid>/status`:

| Field | What |
|-------|------|
| tid, pid | |
| name | as above, per thread (a new thread is named after its creator) |
| state | `running`, `ready`, `blocked` or `zombie` |
| cpu | its home CPU, `-` when it may run anywhere |
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

A CPU's idle time is the time it spent halted (`hlt` / `wfi`): the busy share
over an interval is `1 - Δidle / Δuptime`.

## ps

`ps` (`/bin/custom/ps`, a shell script: `user/ps/ps`) prints a row per
process from these files, by pid: pid, parent, state, threads, virtual
size, CPU time and name. Without options it lists the user processes; `-e`
adds the kernel's (the idle tasks, kernel threads), and `-L` prints a row
per thread instead (its home CPU and CPU time). A process that exits while
`ps` reads it is left out.

```
$ ps
  PID  PPID STATE   THR  SIZE(K)     TIME NAME
    1    11 blocked   1     3004     0:00 netd
    2    11 blocked   1     1468     0:00 sh
   11     0 blocked   1     1168     0:00 init
```

## Memory

`/proc/meminfo` is the frame allocator's counters, `Name: value` lines. A
system monitor reads the last three, in KiB:

| Line | What |
|------|------|
| `MemTotalKiB` | all usable RAM |
| `MemFreeKiB` | the part nothing holds |
| `MemAvailableKiB` | that and the block and page caches, which give their memory back when it runs out |

The others count frames: allocated and freed since boot (`FramesAlloc`,
`FramesFree`, the live ones in `FramesLive` and `LiveKiB`), allocations per
kernel call site (`Site*`, for leak hunting), and the caches' sizes
(`BlockCacheKiB`, `PageCacheKiB`).

## How the time is counted

`task/acct.rs` keeps the counters, plain atomics per task slot and per CPU.
A CPU charges the time since its last charge to its current task on every
`schedule` (a tick, a switch) and before it halts; the halt itself is idle
time, charged to nobody. A task alone on its CPU is so charged at each tick,
and a task halting in `block_until` (it stays the CPU's current task) only up
to the halt. A reading lags a running task by up to a tick (10 ms).

An ending thread hands its time to its process, and `wait` adds the reaped
child's (its own children's included) to the parent's `children`.

## Reading it

Each file is made under one scheduler lock, so a line is consistent in
itself; nothing is consistent across files. A process can exit between
listing `/proc` and opening its `status`: `ENOENT` there is normal, and a
reader drops the process. With 64 task slots a pid is reused within
seconds: a tool that compares two samples keys a process on its pid and its
start time.

Anyone can read every process's status lines. What else of another process
one may see (its fds, namespace, security context) is not decided yet, so
only `/proc/self` shows those.
