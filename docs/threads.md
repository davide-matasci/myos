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

Threads of one process run on the process's home CPU. Every user process
already has one (see `user_affinity` in `task/lifecycle.rs`), and that is
what keeps TLB maintenance local: an address space is only ever loaded on
one CPU, so `munmap`/`mprotect` flush just that CPU. The threads of a
process therefore interleave on it and do not run in parallel; different
processes still spread over the CPUs.

A new thread starts in user mode from a full register image
(`task::UserRegs`, the same one a forked child resumes with): a native thread
at `entry(arg)` on its own stack, a Linux `clone` thread like a forked child
(result 0) on its own stack.

## Lifetime

- `thread_exit` ends the calling thread. A thread is nobody's child: its
  slot is freed once it is off its kernel stack.
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

`user/heap` (`thread_smoke`, `[ OK ] threads` in boot CI) checks shared
memory, wait/wake, and that a process exits while one of its threads sleeps
on an address and another spins in user mode.

## Not yet

- Parallelism within a process (threads of other processes do run on other
  CPUs): it needs TLB shootdowns for shared address spaces.
- Threads in newlib (`pthread_*`) and Rust `std::thread` for native
  programs; the Linux layer runs musl's pthreads (see
  `docs/linux-compat.md`).
- `exec` from a thread other than the leader.
