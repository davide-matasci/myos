# Threads

A myos process can run several threads: tasks that share its address
space, open files, cwd and root, signal dispositions, process group and
session, and exit status, each with its own registers, stacks, thread
pointer (TLS base) and blocked/pending signals.

## Model

| Where | What |
|-------|------|
| `kernel/src/task/thread.rs` | creating threads, thread and process exit, waiting on an address |
| `kernel/src/task/tp.rs` | the user thread pointer per task (x86_64 FS base, aarch64 `tpidr_el0`; riscv64's `tp` is in the trap frame) |
| `kernel/src/task/mod.rs` | `Task::tgid`, `with_process_mut` / `with_thread_mut`, `current_pid` |
| `kernel/src/user/syscall.rs` | `caller_regs`, `thread_start` and the native syscalls below |
| `user/lib/src/thread.rs` | `myos_user::thread`: Rust wrappers |

Every thread is a task slot. The first thread of a process, its leader, is
also the process: the pid is the leader's slot, and the leader's slot holds
what the threads share. Each task has a `tgid`, the slot of its leader
(its own for a single-threaded process), and the shared state is always
reached through it (`with_process_mut`). A thread's id (tid) is its slot.

Every thread gets a home CPU of its own, round-robin like the processes
(`user_affinity` in `task/lifecycle.rs`), so the threads of a process run
in parallel. A new thread first waits on its creator's CPU, which runs the
syscall with interrupts off, until `place_thread` moves it: the Linux
layer's `clone` stores the thread ids the new thread reads first
(`CLONE_PARENT_SETTID`, ...) before it does.

An address space is then loaded on several CPUs. A mapping removed or
narrowed (`munmap`, `mprotect`, `madvise`, a shrinking `brk`) is flushed on
every CPU that has it loaded (`flush_user_tlb`: an IPI shootdown on x86 and
riscv64, the inner-shareable `tlbi` on aarch64), and the frames it unmapped
are freed only after that flush (`free_mapped_page` keeps them until then):
before it, another thread could still reach them through its TLB. A change
that only adds mappings (the heap growing, an `mmap` that is not `MAP_FIXED`)
asks no other CPU to flush on x86_64 and aarch64, which cache no missing
translation (`flush_user_tlb_added`); riscv64 may, and flushes everywhere.

A new thread starts in user mode from a full register image
(`task::UserRegs`, the same one a forked child resumes with): a native thread
at `entry(arg)` on its own stack, a Linux `clone` thread like a forked child
(result 0) on its own stack.

## Lifetime

- `thread_exit` ends the calling thread. A thread is nobody's child: its
  slot is freed once it is off its kernel stack.
- Every thread has a 64 KiB kernel stack (`task::STACK_SIZE`), a heap
  allocation with a canary word at its bottom that `schedule` checks on
  every switch away from it: a thread that ran off its stack panics the
  kernel naming itself (`kernel stack overflow: task N`), instead of
  corrupting whatever the heap placed below, another stack's saved context
  for instance, which would fail much later with a jump to address 0 in a
  task that did nothing wrong. An aarch64 kernel fault report carries the
  link register and the stack pointer at the fault for the same reason.
- The process ends with its last thread. The leader carries the process, so
  it always goes last: a leader whose own thread ends waits for the others,
  then exits the process (closes its fds, reports to the parent, frees the
  address space).
- `exit` (and a fatal signal, or a fault) ends the whole process: the first
  one sets the exit status, the other threads get `SIGKILL`, and the leader
  exits the process once they are gone. A `SIGKILL` also acts when an
  interrupt preempts a thread in user mode (`signal::on_user_preempted`), so
  a thread spinning without syscalls cannot keep its process alive.
- `exec` first ends the process's other threads. Only the leader may exec
  while there are others (Linux lets any thread, which then takes over the
  pid; not supported).
- `fork` from any thread makes a single-threaded child whose thread resumes
  with the calling thread's registers.

## Signals

Dispositions (`sigaction`, ignored signals) belong to the process; the
blocked mask and the pending set belong to each thread. A signal sent to a
pid goes to the leader thread, one sent to a tid to that thread. A signal
whose action is to terminate ends the whole process.

## Waiting on an address

`wait_addr(addr, expected, timeout)` blocks the calling thread while the
32-bit word at `addr` holds `expected`; `wake_addr(addr, n)` wakes up to `n`
threads of the same process waiting on it. Together with atomic operations
in user space they make locks, condition variables and joins (they are what
Linux `futex` maps onto). A wait re-reads the word under the scheduler's
wake sequence, so a wake between the check and the sleep is never lost;
wakes may be spurious, so callers re-check their condition.

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

`user/heap` (`thread_smoke`, `[ OK ] threads` in boot CI) checks shared
memory, wait/wake, and that a process exits while one of its threads sleeps
on an address and another spins in user mode.

## Rust `std::thread`

The std port (`toolchain/std`) runs `std::thread` on these calls:

- **Spawning** (`sys/thread/myos.rs`): a thread gets one mapping: a guard
  page, its stack (1 MiB by default, paged in as it is touched) and a top
  page holding its TLS table and the word its handle waits on. Whoever is
  last frees the mapping: `join` once the thread has ended, or the thread
  itself when its handle was dropped first (detached). A thread ends in a
  few instructions that touch no memory but that word once a joiner may
  free its stack, with its signals blocked so no handler frame lands there.
- **Locks**: std's futex-based `Mutex`, `Condvar`, `RwLock`, `Once` and
  thread parking, on `wait_addr` / `wake_addr` (`pal/myos/futex.rs`). The
  heap (`brk`) takes a `Mutex`.
- **Thread-locals** (`sys/thread_local/key/myos.rs`): a table of 499 keys
  per thread at the thread pointer, its word 0 pointing at itself so x86_64
  code reads it at `fs:0`; the main thread's is a static, installed with
  `set_tp` before `main`. std runs a thread's destructors when it ends.
- `available_parallelism` counts the CPUs of `/proc/cpu`
  (`docs/proc.md`), `yield_now` is `yield`.

The `*-unknown-myos` targets are no longer `singlethread` (which compiled
atomics to plain loads and stores). `/bin/std/thread` (`user/std/thread`, test
`std_thread`) checks the locks, channels, thread-locals and their
destructors, scoped threads, parking, and that joined and detached threads
free their task slots and stacks (120 threads, past the kernel's 64 slots).

## C threads (pthreads)

libgloss (`toolchain/newlib/libgloss/myos/pthread.c`) runs POSIX threads
on the same calls, laid out as the std port's:

- **A thread** is one mapping: a guard page, its stack (1 MiB unless the
  attributes say otherwise, paged in as touched) and on top its control
  block, `struct __pthread`, which `pthread_t` points at and the thread
  pointer too (word 0 points at itself, for x86_64's `fs:0`). Joining frees
  the mapping, or the thread itself when it was detached first, ending in
  the same register-only sequence as a std thread with its signals blocked.
  The main thread's block is a static, and its thread pointer is only set
  when the second thread starts: a program that never starts one runs as
  before. The last thread to end exits the process as `exit(0)` does
  (atexit handlers, stdio flushed); a `pthread_exit` from `main` leaves the
  others running.
- **newlib's state per thread**: newlib is built with `__DYNAMIC_REENT__`
  (`toolchain/newlib/build.sh`), so `errno` and stdio reach their state
  through `__getreent()`, which libgloss answers with the calling thread's
  own `struct _reent` (the main thread's is newlib's `_impure_ptr`). Its
  internal locks (malloc, stdio's streams, atexit, the environment, tz) are
  retargeted (`--enable-newlib-retargetable-locking`) to libgloss mutexes.
- **Locks** are one futex word on `wait_addr` / `wake_addr` (0 free, 1 held,
  2 held with waiters). A mutex keeps its holder's tid and a recursive
  one's count beside it, so `pthread_mutex_t` is four words in the
  sysroot's `<sys/_pthreadtypes.h>` (`build-libgloss.sh`; newlib's own is
  one), and `pthread_t` a pointer. A relock of a default mutex is
  `EDEADLK`, a normal one waits for itself as POSIX says. A condition
  variable is a sequence number its waiters sleep on; `pthread_once` a word
  the others wait on while the first runs the routine.
- **Keys**: 64, values per thread, destructors run when a thread ends. A
  key made again in a deleted key's slot reads `NULL` everywhere.
- **fork** runs the `pthread_atfork` handlers; the child is the forking
  thread alone.

`/bin/etc/pthread_smoke` (test `pthread`) checks the API on one thread,
then a mutex and a condition variable under contention, `errno`, keys,
stdio and malloc per thread, `pthread_once` across threads, `pthread_exit`
with a cleanup handler, a detached thread, and 150 threads started and
joined, past the kernel's task slots.

## Not yet

- C threads: cancellation (a thread is never cancelled), read-write locks,
  barriers and spin locks (newlib declares none of them for myos), a
  condition variable's clock other than `CLOCK_REALTIME`, and newlib's own
  locks held across a fork (issue #333).
- `exec` from a thread other than the leader.
