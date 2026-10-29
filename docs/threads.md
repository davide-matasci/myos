# Threads

A myos process can run several threads. They are tasks that share the
process's address space, open files, cwd and root, signal dispositions,
process group and session, and exit status. Each thread has its own
registers, stacks, thread pointer (TLS base), and blocked and pending
signals. This page covers the kernel's threads, their syscalls, and the
two thread libraries on top: Rust's `std::thread` and libgloss's
pthreads. How tasks are scheduled and block is in `docs/scheduler.md`.

## Model

| Where | What |
|-------|------|
| `kernel/src/task/thread.rs` | creating threads, thread and process exit, waiting on an address |
| `kernel/src/task/tp.rs` | the user thread pointer per task |
| `kernel/src/task/mod.rs` | `Task::tgid`, `with_process_mut` / `with_thread_mut` |
| `kernel/src/user/syscall.rs` | the native syscalls below |
| `user/lib/src/thread.rs` | `myos_user::thread`: Rust wrappers |

Every thread is a task slot, and its id (tid) is its slot. The first
thread, the leader, is also the process: the pid is the leader's tid, and
the leader's slot holds what the threads share. Each task's `tgid` is its
leader's slot, and the kernel reaches the shared state through it. pids
and tids share one number space.

A native thread starts at `entry(arg)` on its own stack; a Linux `clone`
thread starts like a forked child (result 0) on its own stack.

## CPUs and TLBs

Every thread gets a home CPU of its own, round-robin like processes, and
an idle CPU may take it from a busy home later (`docs/scheduler.md`,
"Idle-pull balancing"). So the threads of a process run in parallel. A
new thread first waits on its creator's CPU until the syscall places it;
the Linux layer's `clone` uses that window to store the thread ids the
thread reads first.

An address space loaded on several CPUs shapes the TLB flushes
(`flush_user_tlb` and its callers):

- **Removing or narrowing a mapping** (`munmap`, `mprotect`, `madvise`, a
  shrinking `brk`) flushes every CPU that has the address space loaded.
  The unmapped frames are freed only after that flush, since another
  thread could reach them through its TLB until then.
- **Only adding mappings** flushes no other CPU on x86_64 and aarch64,
  which cache no missing translation; riscv64 may, so it flushes
  everywhere.
- **Widening permissions** records the new protection before rewriting
  the pages, so a page faulted in meanwhile takes it. A permission fault
  on an access the page or region allows comes from a stale translation:
  the kernel fixes the page and retries.

The scheduler keeps a user task loaded on one CPU at a time, so these
rules also cover a task an idle CPU pulls.

## Lifetime

- `thread_exit` ends the calling thread. A thread is nobody's child: its
  slot is freed once it is off its kernel stack.
- The process ends with its last thread. The leader carries the process,
  so it always goes last: a leader whose own thread ends waits for the
  others, then exits the process (fds closed, parent told, address space
  freed).
- `exit`, a fatal signal or a fault ends the whole process. The first one
  sets the exit status and the other threads get `SIGKILL`.
- A signal also acts when an interrupt preempts a thread in user mode, so
  a thread spinning without syscalls cannot keep its process alive.
- `exec` first ends the other threads; only the leader may exec while
  there are others (see "Limits").
- `fork` from any thread makes a single-threaded child that resumes with
  the calling thread's registers.

The kernel has 512 task slots, one per thread, kernel threads and idle
tasks included (`task::MAX_TASKS`, `MYOS_MAX_TASKS` for modules). A
`fork` or `thread_spawn` past them fails (`EAGAIN` in libc). Every thread
has a 64 KiB kernel stack (`task::STACK_SIZE`) with a canary that
`schedule` checks on every switch away: a thread that ran off its stack
panics the kernel naming itself (`kernel stack overflow: task N`) rather
than corrupting a neighbouring stack and failing much later elsewhere.

## Signals

Dispositions belong to the process; the blocked mask and the pending set
to each thread. A signal sent to a pid goes to the leader, one sent to a
tid to that thread, and a terminating action ends the whole process
(`docs/signals.md`).

## Waiting on an address

`wait_addr(addr, expected, timeout)` blocks the calling thread while the
32-bit word at `addr` holds `expected`. `wake_addr(addr, n)` wakes up to
`n` threads of the same process waiting on it. With user-space atomics
they make locks, condition variables and joins; Linux `futex` maps onto
them.

A wait re-reads the word under the scheduler's wake sequence, so a wake
between the check and the sleep is never lost (`docs/scheduler.md`,
"Blocking waits"). That sequence counts every wake in the system, so the
kernel sleeps again while the word still holds `expected` and the timeout
has not passed (issue #520). A wait ends in `0` only on a `wake_addr` for
its address, or when the word changed; so a waker changes the word before
it wakes, as with Linux `futex`, and callers still re-check their
condition.

## Syscalls

| # | Name | Notes |
|--:|------|-------|
| 36 | `getpid()` | the process id (the leader's tid) |
| 53 | `thread_spawn(&ThreadSpawn)` | `{entry, stack, arg, tls}` (four `u64`s): start at `entry(arg)` with the stack pointer at the 16-byte-aligned top `stack` and thread pointer `tls`; returns the tid |
| 54 | `thread_exit(code)` | end the calling thread |
| 55 | `wait_addr(addr, expected, timeout_ns)` | `0` woken, `1` the word differed, `2` timed out (`timeout_ns` 0 = none); `EINTR` on a signal |
| 56 | `wake_addr(addr, count)` | returns how many it woke |
| 57 | `gettid()` | the calling thread's id |
| 89 | `set_tp(value)` | set the calling thread's thread pointer (x86_64 user code cannot write the FS base) |
| 90 | `yield()` | let the other tasks ready on this CPU run first |

`user/heap`'s `thread_smoke` (the `[ OK ] threads` boot line) covers the
kernel side.

## Rust `std::thread`

The std port (`toolchain/std`, `sys/thread/myos.rs`) runs `std::thread`
on these calls. A thread is one mapping: a guard page, its stack (1 MiB
by default, paged in as touched) and a top page with its TLS table and
the word its handle waits on. `join` frees the mapping, or the thread
itself when it was detached first; it then ends in a few instructions
that touch only that word, with signals blocked, so nothing lands on a
stack being freed.

- std's futex-based locks and parking run on `wait_addr` / `wake_addr`
  (`pal/myos/futex.rs`).
- Thread-locals (`sys/thread_local/key/myos.rs`): 499 keys per thread at
  the thread pointer, whose word 0 points at itself (x86_64's `fs:0`).
- `available_parallelism` counts `/proc/cpu` (`docs/proc.md`).

The `*-unknown-myos` targets are not `singlethread`. Test: `std_thread`
(`/bin/std/thread`, `user/std/thread`).

## C threads (pthreads)

libgloss (`toolchain/newlib/libgloss/myos/pthread.c`) runs POSIX threads
on the same calls, laid out like std's: one mapping per thread, with its
control block `struct __pthread` on top, where `pthread_t` and the thread
pointer point. mlibc has its own (`docs/mlibc.md`).

- The main thread's block is a static, and its thread pointer is set only
  when a second thread starts. The last thread to end exits as `exit(0)`
  does, so a `pthread_exit` from `main` leaves the others running.
- newlib is built with `__DYNAMIC_REENT__` (`toolchain/newlib/build.sh`):
  `errno` and stdio state are per thread, and newlib's internal locks are
  retargeted to libgloss mutexes.
- Mutexes are a futex word plus the holder's tid and a recursion count,
  so the sysroot's `pthread_mutex_t` is four words. Condition variables
  take `CLOCK_REALTIME` or `CLOCK_MONOTONIC`. Read-write locks (readers
  first), barriers and spin locks are there too.
- `PTHREAD_PROCESS_SHARED` is `EINVAL`: `wait_addr` keys its words by
  process.
- 64 keys, with destructors.
- Cancellation sends `SIGLOST` (nothing else on myos sends it), with no
  `SA_RESTART`, so a blocking call returns. Deferred cancellation acts at
  the cancellation points (the list is in `pthread.c`); asynchronous
  cancellation acts at the next syscall, so a loop that makes none is not
  interrupted.
- `fork` runs the `pthread_atfork` handlers and holds newlib's own locks
  across the fork, so the child finds malloc and stdio free.
- `_POSIX_THREAD_SAFE_FUNCTIONS`: newlib's `_r` functions, libgloss's
  `getpw*_r`, `getgr*_r`, `readdir_r`, `ttyname_r`, and `flockfile` on
  the stream's own lock. libX11 is built with its locks
  (`packages/x11-libs`).

Test: `pthread` (`/bin/etc/pthread_smoke`, `user/c/pthread_smoke.c`).

## Limits

- Only the leader may `exec` while the process has other threads. Linux
  lets any thread exec, and that thread then takes over the pid; myos
  does not support this yet.
