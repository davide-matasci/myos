// The sysdeps of the dynamic linker (ld.so, docs/mlibc.md "Shared
// objects"): the few calls it makes to find, map and start a program's
// objects, straight on the raw syscalls. ld.so runs before there is a libc
// (it loads libc.so), with no errno and none of the shared C's state, so
// what sysdeps.cpp does over libmyos is spelled out here. Compiled into
// ld.so alone: the static program's linker (libc.a's, which only sets the
// TLS up) and libc.so take sysdeps.cpp's.
#if defined(MLIBC_BUILDING_RTLD) && !defined(MLIBC_STATIC_BUILD)

#include <errno.h>
#include <fcntl.h>
#include <limits.h>
#include <stdint.h>
#include <string.h>
#include <sys/mman.h>
#include <sys/stat.h>
#include <unistd.h>

#include <mlibc/all-sysdeps.hpp>
#include <mlibc/tcb.hpp>

// The raw syscalls, ld.so's own and hidden: an exported function is called
// through the PLT, and ld.so's own PLT is not bound when it starts (it
// relocates itself, the relative relocations alone).
#pragma GCC visibility push(hidden)
extern "C" {
#include "myos_stat.h"
#include "myos_syscalls.h"
#include "myos_raw.c"
}
#pragma GCC visibility pop

namespace {

bool failed(long r) {
	return r == (long)MYOS_SYSERR || r == (long)MYOS_ENAMETOOLONG;
}

} // namespace

namespace mlibc {

void Sysdeps<LibcLog>::operator()(const char *message) {
	myos_syscall6(MYOS_SYS_PWRITE, 2, (long)(uintptr_t)message, (long)strlen(message), 0, 0, 0);
	myos_syscall6(MYOS_SYS_PWRITE, 2, (long)(uintptr_t) "\n", 1, 0, 0, 0);
}

void Sysdeps<LibcPanic>::operator()() {
	sysdep<LibcLog>("ld.so: panic");
	myos_syscall1(MYOS_SYS_EXIT, 134);
	__builtin_unreachable();
}

void Sysdeps<Exit>::operator()(int status) {
	myos_syscall1(MYOS_SYS_EXIT, status);
	__builtin_unreachable();
}

int Sysdeps<AnonAllocate>::operator()(size_t size, void **pointer) {
	return sysdep<VmMap>(nullptr, size, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0, pointer);
}

int Sysdeps<AnonFree>::operator()(void *pointer, size_t size) {
	return sysdep<VmUnmap>(pointer, size);
}

int Sysdeps<VmMap>::operator()(void *hint, size_t size, int prot, int flags, int fd, off_t offset, void **window) {
	struct myos_mmap_args args = {
		(unsigned long)(uintptr_t)hint, size, (unsigned long)prot, (unsigned long)flags, fd, (unsigned long)offset,
	};
	long r = myos_syscall3(MYOS_SYS_MMAP, (long)(uintptr_t)&args, 0, 0);
	if (r == (long)MYOS_SYSERR)
		return ENOMEM;
	*window = (void *)(uintptr_t)r;
	return 0;
}

int Sysdeps<VmUnmap>::operator()(void *pointer, size_t size) {
	if (myos_syscall3(MYOS_SYS_MUNMAP, (long)(uintptr_t)pointer, (long)size, 0) == (long)MYOS_SYSERR)
		return EINVAL;
	return 0;
}

int Sysdeps<VmProtect>::operator()(void *pointer, size_t size, int prot) {
	if (myos_syscall3(MYOS_SYS_MPROTECT, (long)(uintptr_t)pointer, (long)size, prot) == (long)MYOS_SYSERR)
		return EINVAL;
	return 0;
}

int Sysdeps<VmReadahead>::operator()(void *, size_t) {
	return 0;
}

// An object's file, read-only: the flags are the kernel's own access mode.
int Sysdeps<Open>::operator()(const char *pathname, int flags, mode_t, int *fd) {
	long r = myos_syscall6(MYOS_SYS_OPENAT, MYOS_AT_FDCWD, (long)(uintptr_t)pathname, (long)strlen(pathname),
			flags & O_ACCMODE, 0, 0);
	if (failed(r) || r < 0 || r >= MYOS_MAX_FDS)
		return ENOENT;
	*fd = (int)r;
	return 0;
}

int Sysdeps<Close>::operator()(int fd) {
	if (myos_syscall1(MYOS_SYS_CLOSE, fd) == (long)MYOS_SYSERR)
		return EBADF;
	return 0;
}

int Sysdeps<Read>::operator()(int fd, void *buf, size_t count, ssize_t *bytes_read) {
	long r = myos_syscall6(MYOS_SYS_PREAD, fd, (long)(uintptr_t)buf, (long)count, 0, 0, 0);
	if (r < 0 || (size_t)r > count)
		return EIO;
	*bytes_read = r;
	return 0;
}

int Sysdeps<Write>::operator()(int fd, const void *buf, size_t count, ssize_t *bytes_written) {
	long r = myos_syscall6(MYOS_SYS_PWRITE, fd, (long)(uintptr_t)buf, (long)count, 0, 0, 0);
	if (r < 0 || (size_t)r > count)
		return EIO;
	*bytes_written = r;
	return 0;
}

int Sysdeps<Seek>::operator()(int fd, off_t offset, int whence, off_t *new_offset) {
	long r = myos_syscall3(MYOS_SYS_LSEEK, fd, (long)offset, whence);
	if (r == (long)MYOS_SYSERR)
		return ESPIPE;
	*new_offset = r;
	return 0;
}

// What tells two names of one file apart from two files (an object is
// loaded once), and a file's size.
int Sysdeps<Stat>::operator()(fsfd_target fsfdt, int fd, const char *path, int, struct stat *statbuf) {
	struct myos_stat buf;
	long r;
	if (fsfdt == fsfd_target::fd)
		r = myos_syscall6(MYOS_SYS_STATAT, fd, (long)(uintptr_t) "", 0, MYOS_AT_EMPTY_PATH, (long)(uintptr_t)&buf, 0);
	else
		r = myos_syscall6(MYOS_SYS_STATAT, MYOS_AT_FDCWD, (long)(uintptr_t)path, (long)strlen(path), 0,
				(long)(uintptr_t)&buf, 0);
	if (failed(r))
		return ENOENT;
	memset(statbuf, 0, sizeof(*statbuf));
	statbuf->st_mode = buf.st_mode;
	statbuf->st_size = (off_t)buf.st_size;
	statbuf->st_ino = buf.st_ino;
	statbuf->st_dev = (dev_t)buf.st_dev;
	statbuf->st_nlink = buf.st_nlink;
	return 0;
}

int Sysdeps<ClockGet>::operator()(int, time_t *secs, long *nanos) {
	uint64_t ns = (uint64_t)myos_syscall0(MYOS_SYS_CLOCK_MONOTONIC);
	*secs = (time_t)(ns / 1000000000u);
	*nanos = (long)(ns % 1000000000u);
	return 0;
}

int Sysdeps<TcbSet>::operator()(void *pointer) {
#if defined(__x86_64__)
	if (myos_syscall1(MYOS_SYS_SET_TP, (long)(uintptr_t)pointer) == (long)MYOS_SYSERR)
		return EINVAL;
#elif defined(__aarch64__)
	uintptr_t tp = reinterpret_cast<uintptr_t>(pointer) + sizeof(Tcb) - 0x10;
	asm volatile("msr tpidr_el0, %0" ::"r"(tp));
#elif defined(__riscv)
	uintptr_t tp = reinterpret_cast<uintptr_t>(pointer) + sizeof(Tcb);
	asm volatile("mv tp, %0" ::"r"(tp));
#else
#error unsupported arch
#endif
	return 0;
}

pid_t Sysdeps<FutexTid>::operator()() {
	return (pid_t)myos_syscall0(MYOS_SYS_GETTID);
}

// ld.so's own locks, taken before any thread but the first exists and by
// dlopen after: no timeout, and no cancellation to watch for.
int Sysdeps<FutexWait>::operator()(int *pointer, int expected, const struct timespec *) {
	long r = myos_syscall3(MYOS_SYS_WAIT_ADDR, (long)pointer, expected, 0);
	if (r == (long)MYOS_EINTR)
		return EINTR;
	if (r == (long)MYOS_SYSERR)
		return EFAULT;
	return 0;
}

int Sysdeps<FutexWake>::operator()(int *pointer, bool all) {
	if (myos_syscall2(MYOS_SYS_WAKE_ADDR, (long)pointer, all ? INT_MAX : 1) == (long)MYOS_SYSERR)
		return EFAULT;
	return 0;
}

} // namespace mlibc

#endif
