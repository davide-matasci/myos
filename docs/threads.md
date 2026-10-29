# Threads

A myos process can run several threads. They are tasks that share the
process's address space, open files, cwd and root, signal dispositions,
process group and session, and exit status. Each thread has its own
registers, stacks, thread pointer (TLS base), and blocked and pending
signals. This page covers the kernel's threads, the syscalls behind them,
and the two thread libraries on top: Rust's `std::thread` and libgloss's
pthreads. How tasks are scheduled and how they block is in
`docs/scheduler.md`.

## Model

| Where | What |
|-------|------|
| `kernel/src/task/thread.rs` | creating threads, thread and process exit, waiting on an address |
| `kernel/src/task/tp.rs` | the user thread pointer per task (x86_64 FS base, aarch64 `tpidr_el0`; riscv64's `tp` is in the trap frame) |
| `kernel/src/task/mod.rs` | `Task::tgid`, `with_process_mut` / `with_thread_mut`, `current_pid` |
| `kernel/src/user/syscall.rs` | `caller_regs`, `thread_start` and the native syscalls below |
| `user/lib/src/thread.rs` | `myos_user::thread`: Rust wrappers |

Every thread is a task slot, and a thread's id (tid) is its slot. The
first thread of a process, its leader, is also the process: the pid is the
leader's slot, and the leader's slot holds what the threads share. Each
task has a `tgid`, the slot of its leader (its own slot for a
single-threaded process). The kernel always reaches the shared state
through it (`with_process_mut`).

A new thread starts in user mode from a full register image
(`task::UserRegs`, the same one a forked child resumes with):

- a native thread starts at `entry(arg)` on its own stack;
- a Linux `clone` thread starts like a forked child (result 0) on its own
  stack.

## CPUs and TLBs

Every thread gets a home CPU of its own, round-robin like the processes
(`user_affinity` in `kernel/src/task/lifecycle.rs`). An idle CPU may take
it from a busy home later (`docs/scheduler.md`, "Idle-pull balancing").
So the threads of a process run in parallel.

A new thread first waits on its creator's CPU, which runs the syscall with
interrupts off, until `place_thread` moves it to its home. The Linux
layer's `clone` uses that window to store the thread ids the new thread
reads first (`CLONE_PARENT_SETTID`, ...).

An address space is then loaded on several CPUs, which shapes how the
kernel flushes TLBs:

- **Removing or narrowing a mapping** (`munmap`, `mprotect`, `madvise`, a
  shrinking `brk`) is flushed on every CPU that has the address space
  loaded (`flush_user_tlb`). That is an IPI shootdown on x86 and riscv64,
  and the inner-shareable `tlbi` on aarch64. The frames it unmapped are
  freed only after that flush (`free_mapped_page` keeps them until then):
  before it, another thread could still reach them through its TLB.
- **Only adding mappings** (the heap growing, an `mmap` that is not
  `MAP_FIXED`) asks no other CPU to flush on x86_64 and aarch64, which
  cache no missing translation (`flush_user_tlb_added`). riscv64 may cache
  one, so it flushes everywhere.
- **Widening permissions** (`mprotect` to writable or executable) records
  the region's new protection before it rewrites the pages present. A page
  that another thread faults in meanwhile takes the new protection: a
  fault that read the old record reads it again under the fault lock
  (`PROT_GEN`). Every CPU is flushed afterwards.
- **A permission fault** on a fetch or a load whose entry allows the
  access comes from a translation cached before that flush: the kernel
  drops the translation and retries. A permission fault on a page whose
  region allows the access applies the region's protection to the page
  again.

The scheduler keeps a user task loaded on one CPU at a time, so these
rules also cover a task an idle CPU pulls (`docs/scheduler.md`).

## Lifetime

- `thread_exit` ends the calling thread. A thread is nobody's child: its
  slot is freed once it is off its kernel stack.
- The process ends with its last thread. The leader carries the process,
  so it always goes last: a leader whose own thread ends waits for the
  others, then exits the process. That closes its fds, reports to the
  parent and frees the address space.
- `exit`, a fatal signal or a fault ends the whole process. The first one
  sets the exit status, the other threads get `SIGKILL`, and the leader
  exits the process once they are gone.
- A signal also acts when an interrupt preempts a thread in user mode
  (`signal::on_user_interrupt`). So a thread spinning without syscalls
  cannot keep its process alive, and a handler reaches it there.
- `exec` first ends the process's other threads. Only the leader may exec
  while there are others (see "Limits").
- `fork` from any thread makes a single-threaded child, whose thread
  resumes with the calling thread's registers.

### Task slots and kernel stacks

- The kernel has 512 task slots, one per thread, kernel threads and idle
  tasks included (`task::MAX_TASKS`, `MYOS_MAX_TASKS` for modules). A
  `fork` or `thread_spawn` past them fails (`EAGAIN` in libc). Scans of
  every task stop at the highest slot ever used.
- Every thread has a 64 KiB kernel stack (`task::STACK_SIZE`). It is a
  heap allocation with a canary word at its bottom, which `schedule`
  checks on every switch away from the thread. A thread that ran off its
  stack panics the kernel naming itself
  (`kernel stack overflow: task N`). Without the check it would corrupt
  whatever the heap placed below, another stack's saved context for
  instance, and fail much later with a jump to address 0 in a task that
  did nothing wrong.
- For the same reason, an aarch64 kernel fault report carries the link
  register and the stack pointer at the fault.

## Signals

Dispositions (`sigaction`, ignored signals) belong to the process. The
blocked mask and the pending set belong to each thread.

- A signal sent to a pid goes to the leader thread.
- A signal sent to a tid goes to that thread.
- A signal whose action is to terminate ends the whole process.

## Waiting on an address

`wait_addr(addr, expected, timeout)` blocks the calling thread while the
32-bit word at `addr` holds `expected`. `wake_addr(addr, n)` wakes up to
`n` threads of the same process waiting on it. Together with atomic
operations in user space they make locks, condition variables and joins.
Linux `futex` maps onto them.

A wait re-reads the word under the scheduler's wake sequence, so a wake
between the check and the sleep is never lost (`docs/scheduler.md`,
"Blocking waits"). That sequence counts every wake in the system, so a
wake for something else in the same window would end the sleep too. The
kernel then sleeps again while the word still holds `expected` and the
timeout has not passed (issue #520).

A wait ends in `0` only on a `wake_addr` for its address, or when the
word changed while it slept. That is why a waker changes the word before
it wakes, as Linux `futex` asks. Callers still re-check their condition,
as with any futex.

## Syscalls

| # | Name | Notes |
|--:|------|-------|
| 36 | `getpid()` | the process id (the leader's tid) |
| 53 | `thread_spawn(&ThreadSpawn)` | `{entry, stack, arg, tls}` (four `u64`s): start at `entry(arg)` with the stack pointer at the 16-byte-aligned top `stack` and thread pointer `tls`; returns the tid |
| 54 | `thread_exit(code)` | end the calling thread |
| 55 | `wait_addr(addr, expected, timeout_ns)` | `0` woken, `1` the word differed, `2` timed out (`timeout_ns` 0 = none); `EINTR` on a signal |
| 56 | `wake_addr(addr, count)` | returns how many it woke |
| 57 | `gettid()` | the calling thread's id |
| 89 | `set_tp(value)` | make `value` the calling thread's thread pointer, as `thread_spawn`'s `tls` is a new thread's (x86_64 user code cannot write the FS base) |
| 90 | `yield()` | let the other tasks ready on this CPU run first |

`user/heap`'s `thread_smoke` (the `[ OK ] threads` line of the boot
tests) checks shared memory and wait/wake. It also checks that a process
exits while one of its threads sleeps on an address and another spins in
user mode.

## Rust `std::thread`

The std port (`toolchain/std`) runs `std::thread` on these calls:

- **Spawning** (`sys/thread/myos.rs`): a thread gets one mapping. It holds
  a guard page, the thread's stack (1 MiB by default, paged in as it is
  touched) and a top page with its TLS table and the word its handle waits
  on. Whoever is last frees the mapping: `join` once the thread has ended,
  or the thread itself when its handle was dropped first (detached). Once
  a joiner may free its stack, a thread ends in a few instructions that
  touch no memory but that word. Its signals are blocked then, so no
  handler frame lands on the stack.
- **Locks**: std's futex-based `Mutex`, `Condvar`, `RwLock`, `Once` and
  thread parking run on `wait_addr` / `wake_addr` (`pal/myos/futex.rs`).
  The heap (`brk`) takes a `Mutex`.
- **Thread-locals** (`sys/thread_local/key/myos.rs`): a table of 499 keys
  per thread at the thread pointer. Its word 0 points at itself, so x86_64
  code reads it at `fs:0`. The main thread's table is a static, installed
  with `set_tp` before `main`. std runs a thread's destructors when it
  ends.
- `available_parallelism` counts the CPUs of `/proc/cpu` (`docs/proc.md`),
  and `yield_now` is `yield`.

The `*-unknown-myos` targets are no longer `singlethread`, which compiled
atomics to plain loads and stores. `/bin/std/thread` (`user/std/thread`,
test `std_thread`) checks the locks, channels, thread-locals and their
destructors, scoped threads and parking. It also checks that joined and
detached threads free their task slots and stacks (120 threads).

## C threads (pthreads)

libgloss (`toolchain/newlib/libgloss/myos/pthread.c`) runs POSIX threads
on the same calls, laid out like the std port's.

- **A thread** is one mapping: a guard page, its stack (1 MiB unless the
  attributes say otherwise, paged in as touched) and, on top, its control
  block `struct __pthread`. `pthread_t` points at the block, and so does
  the thread pointer (word 0 points at itself, for x86_64's `fs:0`).
  - Joining frees the mapping, or the thread itself does when it was
    detached first. It ends in the same register-only sequence as a std
    thread, with its signals blocked.
  - `pthread_create` detaches a thread created detached only once it has
    stored the thread's id into the block. Before that, the thread could
    end and unmap the block under the store.
  - The main thread's block is a static. Its thread pointer is set only
    when the second thread starts, so a program that never starts one runs
    as before.
  - The last thread to end exits the process as `exit(0)` does (atexit
    handlers run, stdio is flushed). A `pthread_exit` from `main` leaves
    the others running.
- **newlib's state per thread**: newlib is built with `__DYNAMIC_REENT__`
  (`toolchain/newlib/build.sh`). So `errno` and stdio reach their state
  through `__getreent()`, which libgloss answers with the calling thread's
  own `struct _reent` (the main thread's is newlib's `_impure_ptr`).
  newlib's internal locks (malloc, stdio's streams, atexit, the
  environment, tz) are retargeted to libgloss mutexes
  (`--enable-newlib-retargetable-locking`).
- **Mutexes** are one futex word on `wait_addr` / `wake_addr`: 0 free, 1
  held, 2 held with waiters.
  - A mutex keeps its holder's tid, and a recursive one's count, beside
    the word. So `pthread_mutex_t` is four words in the sysroot's
    `<sys/_pthreadtypes.h>` (`build-libgloss.sh`; newlib's own is one),
    and `pthread_t` is a pointer.
  - A relock of a default mutex is `EDEADLK`; a normal one waits for
    itself, as POSIX says.
- **Condition variables** are a sequence number their waiters sleep on,
  and the clock of their timed waits (`pthread_condattr_setclock`:
  `CLOCK_REALTIME` or `CLOCK_MONOTONIC`, the kernel's time since boot).
  `pthread_once` is a word the others wait on while the first thread runs
  the routine.
- **Read-write locks, barriers, spin locks** are declared by the sysroot's
  `features.h`.
  - A read-write lock counts its readers or marks its writer. Its waiters
    sleep on a sequence number that a release moves on. Readers go first,
    so a thread may take its read lock again.
  - A barrier counts the threads of a round under a lock word. The last
    one moves the round on, wakes the others and gets
    `PTHREAD_BARRIER_SERIAL_THREAD`.
  - A spin lock is a word taken by exchange. A spinning thread yields its
    CPU now and then.
  - All three are private to a process, because `wait_addr` keys its
    words by process: `PTHREAD_PROCESS_SHARED` is `EINVAL`.
- **Keys**: 64, with values per thread and destructors run when a thread
  ends. A key made again in a deleted key's slot reads `NULL` everywhere.
- **Cancellation**: `pthread_cancel` marks the thread and sends it
  `SIGLOST`, which nothing else on myos sends. Its handler has no
  `SA_RESTART`, so a blocking call it interrupts returns.
  - Deferred cancellation (the default) ends the thread with
    `PTHREAD_CANCELED` in a cancellation point: `read`, `write` and the
    socket calls over them, `poll`, `select`, `pselect`, `accept`,
    `connect`, `wait`, `waitpid`, `sleep`, `usleep`, `nanosleep`,
    `sigsuspend`, `sigwait`, `pthread_join`, `pthread_testcancel`, and
    the condition waits. A condition wait ends with the mutex locked
    again, as the cleanup handlers expect.
  - Asynchronous cancellation ends the thread at its next syscall:
    signals act on the way out of one (`signals.md`), so a loop that makes
    none is not interrupted.
  - With cancellation disabled the thread blocks the signal, so nothing
    is cut short. Enabled again, the cancellation waits for the next
    cancellation point.
  - Cleanup handlers and key destructors run with cancellation disabled.
- **fork** runs the `pthread_atfork` handlers and holds newlib's own locks
  (atexit, every stream's, the environment, tz, malloc) across the fork.
  So a child forked while another thread is in malloc or stdio finds them
  free. The child is the forking thread alone.
- **Thread-safe functions** (`_POSIX_THREAD_SAFE_FUNCTIONS` in the
  sysroot's `features.h`): newlib's `_r` functions; libgloss's `getpw*_r`,
  `getgr*_r`, `readdir_r` and `ttyname_r`; and `flockfile` /
  `funlockfile` / `ftrylockfile` on the stream's own lock, the one stdio
  takes around each call. libX11 is built with its locks
  (`packages/x11-libs`), which its constructor turns on before `main`.

`/bin/etc/pthread_smoke` (test `pthread`) checks:

- the API on one thread;
- a mutex and a condition variable under contention;
- `errno`, keys, stdio and malloc per thread;
- `pthread_once` across threads;
- `pthread_exit` with a cleanup handler, and a detached thread;
- 150 threads started and joined, and 100 threads alive at once;
- read-write locks, barriers and spin locks under contention;
- a condition wait on `CLOCK_MONOTONIC`;
- cancellation in `read`, in a condition wait, while disabled,
  asynchronous, and of the caller itself;
- forks while two threads keep malloc and stdio busy.

## Limits

- Only the leader may `exec` while the process has other threads. Linux
  lets any thread exec, and that thread then takes over the pid; myos
  does not support this yet.
