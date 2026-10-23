// Threads on the kernel's (docs/threads.md): thread_spawn starts one,
// wait_addr and wake_addr are the futex, and the thread pointer is the
// TCB's address on x86_64 (fs:0 is the TCB's self pointer, set with
// set_tp: user code cannot write the FS base), 16 bytes before the end of
// the TCB on aarch64 and its end on riscv64, as mlibc's get_current_tcb
// reads it (written here directly: tpidr_el0 and tp are user-writable).
#include <errno.h>
#include <limits.h>
#include <stdint.h>
#include <sys/mman.h>

#include <bits/ensure.h>
#include <mlibc/all-sysdeps.hpp>
#include <mlibc/tcb.hpp>

#include "myos_syscalls.h"

namespace {

constexpr size_t PAGE = 4096;
// A thread's stack when the attributes name none (the libgloss default).
constexpr size_t DEFAULT_STACK = 1 << 20;

uintptr_t thread_pointer(void *tcb) {
#if defined(__x86_64__)
	return reinterpret_cast<uintptr_t>(tcb);
#elif defined(__aarch64__)
	return reinterpret_cast<uintptr_t>(tcb) + sizeof(Tcb) - 0x10;
#elif defined(__riscv)
	return reinterpret_cast<uintptr_t>(tcb) + sizeof(Tcb);
#else
#error unsupported arch
#endif
}

} // namespace

extern "C" void __mlibc_thread_entry();

extern "C" void __mlibc_thread_trampoline(void *(*fn)(void *), Tcb *tcb, void *arg) {
	if (mlibc::sysdep<TcbSet>(tcb))
		__ensure(!"failed to set tcb for new thread");

	// The creator stores the tid once thread_spawn returned it.
	while (__atomic_load_n(&tcb->tid, __ATOMIC_RELAXED) == 0)
		mlibc::sysdep<FutexWait>(&tcb->tid, 0, nullptr);

	// Enable cancellation once the TCB is up.
	__atomic_fetch_or(&tcb->cancelBits, tcbCancelEnableBit, __ATOMIC_RELAXED);

	tcb->invokeThreadFunc(reinterpret_cast<void *>(fn), arg);

	mlibc::thread_exit(tcb->returnValue);
}

namespace mlibc {

int Sysdeps<TcbSet>::operator()(void *pointer) {
	uintptr_t tp = thread_pointer(pointer);
#if defined(__x86_64__)
	if (myos_syscall1(MYOS_SYS_SET_TP, (long)tp) == (long)MYOS_SYSERR)
		return EINVAL;
#elif defined(__aarch64__)
	asm volatile("msr tpidr_el0, %0" ::"r"(tp));
#elif defined(__riscv)
	asm volatile("mv tp, %0" ::"r"(tp));
#endif
	return 0;
}

pid_t Sysdeps<FutexTid>::operator()() {
	return (pid_t)myos_syscall0(MYOS_SYS_GETTID);
}

pid_t Sysdeps<GetTid>::operator()() {
	return (pid_t)myos_syscall0(MYOS_SYS_GETTID);
}

// Blocks while *pointer holds `expected`, at most `time` (relative; none
// when null): 0 woken (or the word differed: the callers check again),
// ETIMEDOUT, EINTR on a caught signal.
int Sysdeps<FutexWait>::operator()(int *pointer, int expected, const struct timespec *time) {
	unsigned long long ns = 0;
	if (time != nullptr) {
		if (time->tv_sec < 0 || time->tv_nsec < 0 || time->tv_nsec >= 1000000000L)
			return EINVAL;
		ns = (unsigned long long)time->tv_sec * 1000000000ULL + (unsigned long long)time->tv_nsec;
		if (ns == 0)
			ns = 1; // 0 means no limit to the kernel
	}
	long r = myos_syscall3(MYOS_SYS_WAIT_ADDR, (long)pointer, expected, (long)ns);
	if (r == (long)MYOS_EINTR)
		return EINTR;
	if (r == (long)MYOS_SYSERR)
		return EFAULT;
	return r == 2 ? ETIMEDOUT : 0;
}

int Sysdeps<FutexWake>::operator()(int *pointer, bool all) {
	if (myos_syscall2(MYOS_SYS_WAKE_ADDR, (long)pointer, all ? INT_MAX : 1) == (long)MYOS_SYSERR)
		return EFAULT;
	return 0;
}

// A thread's stack: a mapping of `stack_size` (the default when 0) with a
// guard page at its bottom, or the caller's own; on top the words the
// thread's entry (thread-entry.S) pops: the function, the TCB, the
// argument and a pad that keeps the stack 16-aligned.
int Sysdeps<PrepareStack>::operator()(void **stack, void *entry, void *arg, void *tcb,
		size_t *stack_size, size_t *guard_size, void **stack_base) {
	*stack_size = *stack_size ? (*stack_size + PAGE - 1) & ~(PAGE - 1) : DEFAULT_STACK;
	if (!*stack) {
		*stack_base = mmap(nullptr, *stack_size, PROT_READ | PROT_WRITE, MAP_ANONYMOUS | MAP_PRIVATE, -1, 0);
		if (*stack_base == MAP_FAILED)
			return errno;
		mprotect(*stack_base, PAGE, PROT_NONE);
		*guard_size = PAGE;
	} else {
		*stack_base = *stack;
		*guard_size = 0;
	}
	*stack = (void *)((char *)*stack_base + *stack_size);

	void **stack_it = (void **)*stack;
	*--stack_it = nullptr;
	*--stack_it = arg;
	*--stack_it = tcb;
	*--stack_it = entry;
	*stack = (void *)stack_it;
	return 0;
}

// thread_spawn's {entry, stack, arg, tls}: the thread starts at
// __mlibc_thread_entry on the prepared stack; the kernel sets its thread
// pointer too (the trampoline sets it again, as every port's does).
int Sysdeps<Clone>::operator()(void *tcb, pid_t *tid_out, void *stack) {
	// Cancellation is enabled before the thread runs (the trampoline enables
	// it too, once it does): a pthread_cancel right after pthread_create then
	// sends its signal, instead of leaving a mark for a cancellation point
	// that mlibc's blocking calls do not test (the thread would block in its
	// read for good, and its join with it).
	__atomic_fetch_or(&static_cast<Tcb *>(tcb)->cancelBits, tcbCancelEnableBit, __ATOMIC_RELAXED);
	uint64_t spawn[4] = {
		(uintptr_t)__mlibc_thread_entry, (uintptr_t)stack, 0, thread_pointer(tcb),
	};
	long tid = myos_syscall1(MYOS_SYS_THREAD_SPAWN, (long)spawn);
	if (tid == (long)MYOS_SYSERR)
		return EAGAIN;
	*tid_out = (pid_t)tid;
	return 0;
}

void Sysdeps<ThreadExit>::operator()() {
	myos_syscall1(MYOS_SYS_THREAD_EXIT, 0);
	__builtin_unreachable();
}

void Sysdeps<Yield>::operator()() {
	myos_syscall0(MYOS_SYS_YIELD);
}

} // namespace mlibc
