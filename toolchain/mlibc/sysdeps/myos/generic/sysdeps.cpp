// The sysdeps of mlibc on myos: each a few lines over the C both libcs
// share (libmyos, docs/mlibc.md), which answers as newlib's libgloss does
// (-1 with errno), turned into the errno value mlibc's calls take. The
// ones mlibc's startup and allocator need before the TLS is up (the
// logger, the anonymous memory, the thread pointer) use the raw syscalls.
#include <errno.h>
#include <fcntl.h>
#include <ifaddrs.h>
#include <limits.h>
#include <net/if.h>
#include <poll.h>
#include <pty.h>
#include <stdarg.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/epoll.h>
#include <sys/eventfd.h>
#include <sys/ioctl.h>
#include <sys/mman.h>
#include <sys/reboot.h>
#include <sys/resource.h>
#include <sys/signalfd.h>
#include <sys/socket.h>
#include <sys/stat.h>
#include <sys/statvfs.h>
#include <sys/time.h>
#include <sys/timerfd.h>
#include <sys/times.h>
#include <sys/uio.h>
#include <sys/utsname.h>
#include <sys/wait.h>
#include <termios.h>
#include <unistd.h>

#include <bits/ensure.h>
#include <mlibc/all-sysdeps.hpp>
#include <mlibc/debug.hpp>

#include "libmyos.h"

namespace {

// The errno of a call that answered -1, or 0.
int err(long r) {
	return r < 0 ? errno : 0;
}

// The flags a new fd takes after the call that made it: O_CLOEXEC and
// O_NONBLOCK (or their SOCK_ spellings) are applied with fcntl.
int apply_flags(int fd, int cloexec, int nonblock) {
	if (cloexec && __myos_fcntl(fd, F_SETFD, FD_CLOEXEC) < 0)
		return errno;
	if (nonblock && __myos_fcntl(fd, F_SETFL, O_NONBLOCK) < 0)
		return errno;
	return 0;
}

} // namespace

namespace mlibc {

// ---- the startup's and allocator's ------------------------------------

void Sysdeps<LibcLog>::operator()(const char *message) {
	myos_syscall6(MYOS_SYS_PWRITE, 2, (long)(uintptr_t)message, (long)strlen(message), 0, 0, 0);
	myos_syscall6(MYOS_SYS_PWRITE, 2, (long)(uintptr_t) "\n", 1, 0, 0, 0);
}

void Sysdeps<LibcPanic>::operator()() {
	sysdep<LibcLog>("mlibc: panic");
	myos_syscall1(MYOS_SYS_EXIT, 134);
	__builtin_unreachable();
}

void Sysdeps<Exit>::operator()(int status) {
	__myos_exit(status);
}

int Sysdeps<AnonAllocate>::operator()(size_t size, void **pointer) {
	struct myos_mmap_args args = {0, size, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0};
	long r = myos_syscall3(MYOS_SYS_MMAP, (long)(uintptr_t)&args, 0, 0);
	if (r == (long)MYOS_SYSERR)
		return ENOMEM;
	*pointer = (void *)(uintptr_t)r;
	return 0;
}

int Sysdeps<AnonFree>::operator()(void *pointer, size_t size) {
	if (myos_syscall3(MYOS_SYS_MUNMAP, (long)(uintptr_t)pointer, (long)size, 0) == (long)MYOS_SYSERR)
		return EINVAL;
	return 0;
}

int Sysdeps<VmMap>::operator()(void *hint, size_t size, int prot, int flags, int fd, off_t offset, void **window) {
	void *p = __myos_mmap(hint, size, prot, flags, fd, offset);
	if (p == MAP_FAILED)
		return errno;
	*window = p;
	return 0;
}

int Sysdeps<VmUnmap>::operator()(void *pointer, size_t size) {
	return err(__myos_munmap(pointer, size));
}

int Sysdeps<VmProtect>::operator()(void *pointer, size_t size, int prot) {
	return err(__myos_mprotect(pointer, size, prot));
}

int Sysdeps<Msync>::operator()(void *addr, size_t length, int flags) {
	return err(__myos_msync(addr, length, flags));
}

// No paging advice to give: every page is resident.
int Sysdeps<Madvise>::operator()(void *, size_t, int) {
	return 0;
}

int Sysdeps<PosixMadvise>::operator()(void *, size_t, int) {
	return 0;
}

int Sysdeps<Mlock>::operator()(const void *, size_t) {
	return 0;
}

int Sysdeps<Munlock>::operator()(const void *, size_t) {
	return 0;
}

int Sysdeps<Mlockall>::operator()(int) {
	return 0;
}

int Sysdeps<Munlockall>::operator()() {
	return 0;
}

#if MLIBC_BUILDING_RTLD
int Sysdeps<VmReadahead>::operator()(void *, size_t) {
	return 0;
}
#endif

int Sysdeps<Brk>::operator()(void **out) {
	void *cur = __myos_sbrk(0);
	if (cur == (void *)-1)
		return errno;
	*out = cur;
	return 0;
}

// ---- files and fds ------------------------------------------------------

int Sysdeps<Open>::operator()(const char *pathname, int flags, mode_t mode, int *fd) {
	return sysdep<Openat>(AT_FDCWD, pathname, flags, mode, fd);
}

int Sysdeps<Openat>::operator()(int dirfd, const char *path, int flags, mode_t, int *fd) {
	int r = __myos_openat(dirfd, path, flags);
	if (r < 0)
		return errno;
	*fd = r;
	return 0;
}

int Sysdeps<Close>::operator()(int fd) {
	return err(__myos_close(fd));
}

int Sysdeps<Read>::operator()(int fd, void *buf, size_t count, ssize_t *bytes_read) {
	ssize_t n = __myos_read(fd, buf, count);
	if (n < 0)
		return errno;
	*bytes_read = n;
	return 0;
}

int Sysdeps<Write>::operator()(int fd, const void *buf, size_t count, ssize_t *bytes_written) {
	ssize_t n = __myos_write(fd, buf, count);
	if (n < 0)
		return errno;
	*bytes_written = n;
	return 0;
}

int Sysdeps<Pread>::operator()(int fd, void *buf, size_t n, off_t off, ssize_t *bytes_read) {
	ssize_t got = __myos_pread(fd, buf, n, off);
	if (got < 0)
		return errno;
	*bytes_read = got;
	return 0;
}

int Sysdeps<Pwrite>::operator()(int fd, const void *buf, size_t n, off_t off, ssize_t *bytes_written) {
	ssize_t put = __myos_pwrite(fd, buf, n, off);
	if (put < 0)
		return errno;
	*bytes_written = put;
	return 0;
}

int Sysdeps<Readv>::operator()(int fd, const struct iovec *iovs, int iovc, ssize_t *bytes_read) {
	ssize_t n = __myos_readv(fd, iovs, iovc);
	if (n < 0)
		return errno;
	*bytes_read = n;
	return 0;
}

int Sysdeps<Writev>::operator()(int fd, const struct iovec *iovs, int iovc, ssize_t *bytes_written) {
	ssize_t n = __myos_writev(fd, iovs, iovc);
	if (n < 0)
		return errno;
	*bytes_written = n;
	return 0;
}

int Sysdeps<Seek>::operator()(int fd, off_t offset, int whence, off_t *new_offset) {
	off_t r = __myos_lseek(fd, offset, whence);
	if (r < 0)
		return errno;
	*new_offset = r;
	return 0;
}

int Sysdeps<Dup>::operator()(int fd, int flags, int *newfd) {
	int r = __myos_fcntl(fd, (flags & O_CLOEXEC) ? F_DUPFD_CLOEXEC : F_DUPFD, 0);
	if (r < 0)
		return errno;
	*newfd = r;
	return 0;
}

int Sysdeps<Dup2>::operator()(int fd, int flags, int newfd) {
	if (fd == newfd)
		return flags ? EINVAL : 0;
	if (__myos_dup2(fd, newfd) < 0)
		return errno;
	return apply_flags(newfd, flags & O_CLOEXEC, 0);
}

int Sysdeps<Fcntl>::operator()(int fd, int request, va_list args, int *result) {
	long arg = va_arg(args, long);
	int r = __myos_fcntl(fd, request, arg);
	if (r < 0)
		return errno;
	*result = r;
	return 0;
}

int Sysdeps<Isatty>::operator()(int fd) {
	return myos_fd_is_tty(fd) ? 0 : ENOTTY;
}

int Sysdeps<Ttyname>::operator()(int fd, char *buf, size_t size) {
	return __myos_ttyname_r(fd, buf, size);
}

// The fd's /proc/self/fd link, for the callers that read what it names.
int Sysdeps<FdToPath>::operator()(int fd, char **out) {
	char *path = (char *)malloc(32);
	if (path == nullptr)
		return ENOMEM;
	snprintf(path, 32, "/proc/self/fd/%d", fd);
	*out = path;
	return 0;
}

int Sysdeps<Stat>::operator()(fsfd_target fsfdt, int fd, const char *path, int flags, struct stat *statbuf) {
	int r;
	switch (fsfdt) {
	case fsfd_target::path:
		r = __myos_fstatat(AT_FDCWD, path, statbuf, flags);
		break;
	case fsfd_target::fd:
		r = __myos_fstat(fd, statbuf);
		break;
	case fsfd_target::fd_path:
		r = __myos_fstatat(fd, path, statbuf, flags);
		break;
	default:
		return EINVAL;
	}
	return err(r);
}

int Sysdeps<Access>::operator()(const char *path, int mode) {
	return err(__myos_access(path, mode));
}

int Sysdeps<Faccessat>::operator()(int dirfd, const char *pathname, int mode, int flags) {
	return err(__myos_faccessat(dirfd, pathname, mode, flags));
}

int Sysdeps<Readlink>::operator()(const char *path, void *buffer, size_t max_size, ssize_t *length) {
	return sysdep<Readlinkat>(AT_FDCWD, path, buffer, max_size, length);
}

int Sysdeps<Readlinkat>::operator()(int dirfd, const char *path, void *buffer, size_t max_size, ssize_t *length) {
	ssize_t n = __myos_readlinkat(dirfd, path, (char *)buffer, max_size);
	if (n < 0)
		return errno;
	*length = n;
	return 0;
}

int Sysdeps<Mkdir>::operator()(const char *path, mode_t mode) {
	return err(__myos_mkdir(path, mode));
}

int Sysdeps<Mkdirat>::operator()(int dirfd, const char *path, mode_t mode) {
	return err(__myos_mkdirat(dirfd, path, mode));
}

int Sysdeps<Mkfifoat>::operator()(int dirfd, const char *path, mode_t mode) {
	return err(__myos_mkfifoat(dirfd, path, mode));
}

int Sysdeps<Mknodat>::operator()(int dirfd, const char *path, int mode, int dev) {
	return err(__myos_mknodat(dirfd, path, (mode_t)mode, (dev_t)dev));
}

int Sysdeps<Symlink>::operator()(const char *target_path, const char *link_path) {
	return err(__myos_symlink(target_path, link_path));
}

int Sysdeps<Symlinkat>::operator()(const char *target_path, int dirfd, const char *link_path) {
	return err(__myos_symlinkat(target_path, dirfd, link_path));
}

int Sysdeps<Link>::operator()(const char *old_path, const char *new_path) {
	return err(__myos_link(old_path, new_path));
}

int Sysdeps<Linkat>::operator()(int olddirfd, const char *old_path, int newdirfd, const char *new_path, int flags) {
	return err(__myos_linkat(olddirfd, old_path, newdirfd, new_path, flags));
}

int Sysdeps<Unlinkat>::operator()(int dirfd, const char *path, int flags) {
	return err(__myos_unlinkat(dirfd, path, flags));
}

int Sysdeps<Rmdir>::operator()(const char *path) {
	return err(__myos_rmdir(path));
}

int Sysdeps<Rename>::operator()(const char *path, const char *new_path) {
	return err(__myos_rename(path, new_path));
}

int Sysdeps<Renameat>::operator()(int olddirfd, const char *old_path, int newdirfd, const char *new_path) {
	return err(__myos_renameat(olddirfd, old_path, newdirfd, new_path));
}

int Sysdeps<Truncate>::operator()(const char *path, off_t length) {
	int fd = __myos_open(path, O_WRONLY);
	if (fd < 0)
		return errno;
	int e = err(__myos_ftruncate(fd, length));
	__myos_close(fd);
	return e;
}

int Sysdeps<Ftruncate>::operator()(int fd, size_t size) {
	return err(__myos_ftruncate(fd, (off_t)size));
}

int Sysdeps<Utimensat>::operator()(int dirfd, const char *pathname, const struct timespec times[2], int flags) {
	return err(__myos_utimensat(dirfd, pathname, times, flags));
}

int Sysdeps<Chmod>::operator()(const char *pathname, mode_t mode) {
	return err(__myos_chmod(pathname, mode));
}

int Sysdeps<Fchmod>::operator()(int fd, mode_t mode) {
	return err(__myos_fchmod(fd, mode));
}

int Sysdeps<Fchmodat>::operator()(int fd, const char *pathname, mode_t mode, int flags) {
	return err(__myos_fchmodat(fd, pathname, mode, flags));
}

int Sysdeps<Fchownat>::operator()(int dirfd, const char *pathname, uid_t owner, gid_t group, int flags) {
	return err(__myos_fchownat(dirfd, pathname, owner, group, flags));
}

int Sysdeps<Umask>::operator()(mode_t mode, mode_t *old) {
	static mode_t cur = 022;
	*old = cur;
	cur = mode;
	return 0;
}

// tmpfs and ext2 writes are synchronous from userspace's point of view.
int Sysdeps<Fsync>::operator()(int fd) {
	struct stat st;
	return err(__myos_fstat(fd, &st));
}

int Sysdeps<Fdatasync>::operator()(int fd) {
	return sysdep<Fsync>(fd);
}

void Sysdeps<Sync>::operator()() {}

int Sysdeps<Fadvise>::operator()(int, off_t, off_t, int) {
	return 0;
}

int Sysdeps<Flock>::operator()(int fd, int options) {
	return err(__myos_flock(fd, options));
}

int Sysdeps<Pipe>::operator()(int *fds, int flags) {
	if (myos_pipe_flags(fds, flags & O_CLOEXEC) < 0)
		return errno;
	if (flags & O_NONBLOCK) {
		__myos_fcntl(fds[0], F_SETFL, O_NONBLOCK);
		__myos_fcntl(fds[1], F_SETFL, O_NONBLOCK);
	}
	return 0;
}

int Sysdeps<Statvfs>::operator()(const char *path, struct statvfs *out) {
	return err(__myos_statvfs(path, out));
}

int Sysdeps<Fstatvfs>::operator()(int fd, struct statvfs *out) {
	return err(__myos_fstatvfs(fd, out));
}

int Sysdeps<Ioctl>::operator()(int fd, unsigned long request, void *arg, int *result) {
	int r = __myos_ioctl(fd, request, arg);
	if (r < 0)
		return errno;
	if (result != nullptr)
		*result = r;
	return 0;
}

int Sysdeps<GetCwd>::operator()(char *buffer, size_t size) {
	return __myos_getcwd(buffer, size) == nullptr ? errno : 0;
}

int Sysdeps<Chdir>::operator()(const char *path) {
	return err(__myos_chdir(path));
}

int Sysdeps<Fchdir>::operator()(int fd) {
	return err(__myos_fchdir(fd));
}

int Sysdeps<Chroot>::operator()(const char *path) {
	return err(__myos_chroot(path));
}

// ---- waiting and the clocks ---------------------------------------------

int Sysdeps<Poll>::operator()(struct pollfd *fds, nfds_t count, int timeout, int *num_events) {
	int n = __myos_poll(fds, count, timeout);
	if (n < 0)
		return errno;
	*num_events = n;
	return 0;
}

// The mask is set and restored around the poll, as libmyos's pselect does
// with its own.
int Sysdeps<Ppoll>::operator()(struct pollfd *fds, nfds_t count, const struct timespec *ts, const sigset_t *mask, int *num_events) {
	int timeout = -1;
	if (ts != nullptr) {
		if (ts->tv_sec < 0 || ts->tv_nsec < 0 || ts->tv_nsec >= 1000000000L)
			return EINVAL;
		long long ms = (long long)ts->tv_sec * 1000 + (ts->tv_nsec + 999999) / 1000000;
		timeout = ms > INT_MAX ? INT_MAX : (int)ms;
	}
	sigset_t old;
	if (mask != nullptr) {
		if (int e = sysdep<Sigprocmask>(SIG_SETMASK, mask, &old); e)
			return e;
	}
	int n = __myos_poll(fds, count, timeout);
	int e = n < 0 ? errno : 0;
	if (mask != nullptr)
		sysdep<Sigprocmask>(SIG_SETMASK, &old, nullptr);
	if (e)
		return e;
	*num_events = n;
	return 0;
}

int Sysdeps<Pselect>::operator()(int num_fds, fd_set *read_set, fd_set *write_set, fd_set *except_set,
		const struct timespec *timeout, const sigset_t *sigmask, int *num_events) {
	int n = __myos_pselect(num_fds, read_set, write_set, except_set, timeout, sigmask);
	if (n < 0)
		return errno;
	*num_events = n;
	return 0;
}

// A caught signal ends the sleep: what is left goes back to the caller.
int Sysdeps<Sleep>::operator()(time_t *secs, long *nanos) {
	struct timespec req = {*secs, *nanos};
	struct timespec rem = {0, 0};
	if (__myos_nanosleep(&req, &rem) < 0) {
		int e = errno;
		*secs = rem.tv_sec;
		*nanos = rem.tv_nsec;
		return e;
	}
	*secs = 0;
	*nanos = 0;
	return 0;
}

int Sysdeps<ClockGet>::operator()(int clock, time_t *secs, long *nanos) {
	struct timespec tp;
	if (__myos_clock_gettime(clock, &tp) < 0)
		return errno;
	*secs = tp.tv_sec;
	*nanos = tp.tv_nsec;
	return 0;
}

int Sysdeps<ClockSet>::operator()(int clock, time_t secs, long nanos) {
	struct timespec tp = {secs, nanos};
	return err(__myos_clock_settime(clock, &tp));
}

int Sysdeps<ClockGetres>::operator()(int clock, time_t *secs, long *nanos) {
	struct timespec res;
	if (__myos_clock_getres(clock, &res) < 0)
		return errno;
	*secs = res.tv_sec;
	*nanos = res.tv_nsec;
	return 0;
}

int Sysdeps<SetItimer>::operator()(int which, const struct itimerval *new_value, struct itimerval *old_value) {
	return err(__myos_setitimer(which, new_value, old_value));
}

int Sysdeps<GetItimer>::operator()(int which, struct itimerval *curr_value) {
	return err(__myos_getitimer(which, curr_value));
}

// No CPU time accounting (libgloss's times answers the same).
int Sysdeps<Times>::operator()(struct tms *, clock_t *) {
	return ENOSYS;
}

int Sysdeps<GetRusage>::operator()(int, struct rusage *usage) {
	memset(usage, 0, sizeof *usage);
	return 0;
}

// ---- processes ----------------------------------------------------------

int Sysdeps<Fork>::operator()(pid_t *child) {
	int r = __myos_fork();
	if (r < 0)
		return errno;
	*child = r;
	return 0;
}

int Sysdeps<Execve>::operator()(const char *path, char *const argv[], char *const envp[]) {
	__myos_execve(path, argv, envp);
	return errno;
}

int Sysdeps<Fexecve>::operator()(int fd, char *const argv[], char *const envp[]) {
	__myos_fexecve(fd, argv, envp);
	return errno;
}

int Sysdeps<Waitpid>::operator()(pid_t pid, int *status, int flags, struct rusage *ru, pid_t *ret_pid) {
	pid_t r = __myos_waitpid(pid, status, flags);
	if (r < 0)
		return errno;
	if (ru != nullptr)
		memset(ru, 0, sizeof *ru);
	*ret_pid = r;
	return 0;
}

pid_t Sysdeps<GetPid>::operator()() {
	return __myos_getpid();
}

pid_t Sysdeps<GetPpid>::operator()() {
	return __myos_getppid();
}

int Sysdeps<GetPgid>::operator()(pid_t pid, pid_t *pgid) {
	pid_t r = __myos_getpgid(pid);
	if (r < 0)
		return errno;
	*pgid = r;
	return 0;
}

int Sysdeps<GetSid>::operator()(pid_t pid, pid_t *sid) {
	pid_t r = __myos_getsid(pid);
	if (r < 0)
		return errno;
	*sid = r;
	return 0;
}

int Sysdeps<SetPgid>::operator()(pid_t pid, pid_t pgid) {
	return err(__myos_setpgid(pid, pgid));
}

int Sysdeps<SetSid>::operator()(pid_t *sid) {
	int r = __myos_setsid();
	if (r < 0)
		return errno;
	*sid = r;
	return 0;
}

// The users are the security policy's (docs/security.md): there is one
// group, root, and the ids cannot be changed by a call (sec's `as`
// changes the user through myos_setuser).
uid_t Sysdeps<GetUid>::operator()() {
	return __myos_getuid();
}

uid_t Sysdeps<GetEuid>::operator()() {
	return __myos_geteuid();
}

gid_t Sysdeps<GetGid>::operator()() {
	return 0;
}

gid_t Sysdeps<GetEgid>::operator()() {
	return 0;
}

int Sysdeps<SetUid>::operator()(uid_t) {
	return 0;
}

int Sysdeps<SetEuid>::operator()(uid_t) {
	return 0;
}

int Sysdeps<SetGid>::operator()(gid_t) {
	return 0;
}

int Sysdeps<SetEgid>::operator()(gid_t) {
	return 0;
}

int Sysdeps<GetGroups>::operator()(size_t, gid_t *, int *ret) {
	*ret = 0;
	return 0;
}

int Sysdeps<SetGroups>::operator()(size_t, const gid_t *) {
	return 0;
}

int Sysdeps<GetLoginR>::operator()(char *name, size_t name_len) {
	return __myos_getlogin_r(name, name_len);
}

int Sysdeps<GetPriority>::operator()(int, id_t, int *value) {
	*value = 0;
	return 0;
}

int Sysdeps<SetPriority>::operator()(int, id_t, int) {
	return ENOSYS;
}

int Sysdeps<Sysconf>::operator()(int num, long *ret) {
	switch (num) {
	case _SC_OPEN_MAX:
		*ret = MYOS_OPEN_MAX;
		return 0;
	case _SC_PAGESIZE:
		*ret = 4096;
		return 0;
	case _SC_GETPW_R_SIZE_MAX:
	case _SC_GETGR_R_SIZE_MAX:
		*ret = 1024;
		return 0;
	default:
		return EINVAL;
	}
}

// ---- the host -----------------------------------------------------------

int Sysdeps<GetHostname>::operator()(char *buffer, size_t bufsize) {
	return err(__myos_gethostname(buffer, bufsize));
}

int Sysdeps<SetHostname>::operator()(const char *buffer, size_t bufsize) {
	return err(__myos_sethostname(buffer, bufsize));
}

int Sysdeps<Uname>::operator()(struct utsname *buf) {
	return err(__myos_uname(buf));
}

// The kernel's generator (/dev/urandom) never blocks.
int Sysdeps<GetEntropy>::operator()(void *buffer, size_t length) {
	int fd = __myos_open("/dev/urandom", O_RDONLY | O_CLOEXEC);
	if (fd < 0)
		return errno;
	size_t got = 0;
	while (got < length) {
		ssize_t n = __myos_read(fd, (char *)buffer + got, length - got);
		if (n < 0 && errno == EINTR)
			continue;
		if (n <= 0) {
			int e = n < 0 ? errno : EIO;
			__myos_close(fd);
			return e;
		}
		got += (size_t)n;
	}
	__myos_close(fd);
	return 0;
}

int Sysdeps<Reboot>::operator()(int cmd) {
	return err(__myos_reboot(cmd));
}

// ---- terminals ----------------------------------------------------------

int Sysdeps<Tcgetattr>::operator()(int fd, struct termios *attr) {
	return err(__myos_tcgetattr(fd, attr));
}

int Sysdeps<Tcsetattr>::operator()(int fd, int optional_action, const struct termios *attr) {
	return err(__myos_tcsetattr(fd, optional_action, attr));
}

int Sysdeps<Tcsendbreak>::operator()(int fd, int dur) {
	return err(__myos_tcsendbreak(fd, dur));
}

int Sysdeps<Tcflow>::operator()(int fd, int action) {
	return err(__myos_tcflow(fd, action));
}

int Sysdeps<Tcflush>::operator()(int fd, int queue) {
	return err(__myos_tcflush(fd, queue));
}

int Sysdeps<Tcdrain>::operator()(int fd) {
	return err(__myos_tcdrain(fd));
}

int Sysdeps<Tcgetwinsize>::operator()(int fd, struct winsize *winsz) {
	return err(myos_tty_get(fd, nullptr, winsz));
}

int Sysdeps<Tcsetwinsize>::operator()(int fd, const struct winsize *winsz) {
	return err(myos_tty_set_winsize(fd, winsz->ws_row, winsz->ws_col));
}

int Sysdeps<Openpty>::operator()(int *mfd, int *sfd, char *name, const struct termios *ios, const struct winsize *win) {
	return err(__myos_openpty(mfd, sfd, name, ios, win));
}

// A pty pair is a directory, /dev/pts/N (docs/tty.md): the master's
// /proc/self/fd link names it, and the slave is its data.
int Sysdeps<Openpt>::operator()(int oflags, int *fd) {
	int r = __myos_open("/dev/pts/clone", oflags);
	if (r < 0)
		return errno;
	*fd = r;
	return 0;
}

int Sysdeps<Ptsname>::operator()(int fd, char *buffer, size_t length) {
	char dir[MYOS_TTY_PATH];
	int master;
	if (myos_tty_dir(fd, dir, sizeof dir, &master) != 0)
		return errno;
	if (!master)
		return ENOTTY;
	if ((size_t)snprintf(buffer, length, "%s/data", dir) >= length)
		return ERANGE;
	return 0;
}

// No lock to lift: the slave opens as soon as the pair exists.
int Sysdeps<Unlockpt>::operator()(int fd) {
	return myos_tty_dir(fd, nullptr, 0, nullptr) != 0 ? errno : 0;
}

// ---- sockets ------------------------------------------------------------

int Sysdeps<Socket>::operator()(int family, int type, int protocol, int *fd) {
	int r = __myos_socket(family, type & ~(SOCK_NONBLOCK | SOCK_CLOEXEC), protocol);
	if (r < 0)
		return errno;
	if (int e = apply_flags(r, type & SOCK_CLOEXEC, type & SOCK_NONBLOCK); e) {
		__myos_close(r);
		return e;
	}
	*fd = r;
	return 0;
}

int Sysdeps<Socketpair>::operator()(int domain, int type_and_flags, int proto, int *fds) {
	if (__myos_socketpair(domain, type_and_flags & ~(SOCK_NONBLOCK | SOCK_CLOEXEC), proto, fds) < 0)
		return errno;
	for (int i = 0; i < 2; i++) {
		if (int e = apply_flags(fds[i], type_and_flags & SOCK_CLOEXEC, type_and_flags & SOCK_NONBLOCK); e) {
			__myos_close(fds[0]);
			__myos_close(fds[1]);
			return e;
		}
	}
	return 0;
}

int Sysdeps<Bind>::operator()(int fd, const struct sockaddr *addr_ptr, socklen_t addr_length) {
	return err(__myos_bind(fd, addr_ptr, addr_length));
}

int Sysdeps<Listen>::operator()(int fd, int backlog) {
	return err(__myos_listen(fd, backlog));
}

int Sysdeps<Connect>::operator()(int fd, const struct sockaddr *addr_ptr, socklen_t addr_length) {
	return err(__myos_connect(fd, addr_ptr, addr_length));
}

int Sysdeps<Accept>::operator()(int fd, int *newfd, struct sockaddr *addr_ptr, socklen_t *addr_length, int flags) {
	int r = __myos_accept(fd, addr_ptr, addr_length);
	if (r < 0)
		return errno;
	if (int e = apply_flags(r, flags & SOCK_CLOEXEC, flags & SOCK_NONBLOCK); e) {
		__myos_close(r);
		return e;
	}
	*newfd = r;
	return 0;
}

int Sysdeps<Shutdown>::operator()(int sockfd, int how) {
	return err(__myos_shutdown(sockfd, how));
}

int Sysdeps<Sockname>::operator()(int fd, struct sockaddr *addr_ptr, socklen_t max_addr_length, socklen_t *actual_length) {
	socklen_t len = max_addr_length;
	if (__myos_getsockname(fd, addr_ptr, &len) < 0)
		return errno;
	*actual_length = len;
	return 0;
}

int Sysdeps<Peername>::operator()(int fd, struct sockaddr *addr_ptr, socklen_t max_addr_length, socklen_t *actual_length) {
	socklen_t len = max_addr_length;
	if (__myos_getpeername(fd, addr_ptr, &len) < 0)
		return errno;
	*actual_length = len;
	return 0;
}

int Sysdeps<MsgSend>::operator()(int fd, const struct msghdr *hdr, int flags, ssize_t *length) {
	ssize_t n = __myos_sendmsg(fd, hdr, flags);
	if (n < 0)
		return errno;
	*length = n;
	return 0;
}

int Sysdeps<MsgRecv>::operator()(int fd, struct msghdr *hdr, int flags, ssize_t *length) {
	ssize_t n = __myos_recvmsg(fd, hdr, flags);
	if (n < 0)
		return errno;
	*length = n;
	return 0;
}

int Sysdeps<Sendto>::operator()(int fd, const void *buffer, size_t size, int flags, const struct sockaddr *sock_addr, socklen_t addr_length, ssize_t *length) {
	ssize_t n = __myos_sendto(fd, buffer, size, flags, sock_addr, addr_length);
	if (n < 0)
		return errno;
	*length = n;
	return 0;
}

int Sysdeps<Recvfrom>::operator()(int fd, void *buffer, size_t size, int flags, struct sockaddr *sock_addr, socklen_t *addr_length, ssize_t *length) {
	ssize_t n = __myos_recvfrom(fd, buffer, size, flags, sock_addr, addr_length);
	if (n < 0)
		return errno;
	*length = n;
	return 0;
}

int Sysdeps<GetSockopt>::operator()(int fd, int layer, int number, void *__restrict buffer, socklen_t *__restrict size) {
	return err(__myos_getsockopt(fd, layer, number, buffer, size));
}

int Sysdeps<SetSockopt>::operator()(int fd, int layer, int number, const void *buffer, socklen_t size) {
	return err(__myos_setsockopt(fd, layer, number, buffer, size));
}

int Sysdeps<InetConfigured>::operator()(bool *ipv4, bool *ipv6) {
	*ipv4 = true;
	*ipv6 = false;
	return 0;
}

int Sysdeps<IfNametoindex>::operator()(const char *name, unsigned int *ret) {
	unsigned int r = __myos_if_nametoindex(name);
	if (r == 0)
		return errno ? errno : ENXIO;
	*ret = r;
	return 0;
}

int Sysdeps<IfIndextoname>::operator()(unsigned int index, char *name) {
	return __myos_if_indextoname(index, name) == nullptr ? errno : 0;
}

int Sysdeps<IfNameindex>::operator()(struct if_nameindex **out) {
	struct if_nameindex *r = __myos_if_nameindex();
	if (r == nullptr)
		return errno;
	*out = r;
	return 0;
}

void Sysdeps<IfFreeNameindex>::operator()(struct if_nameindex *ifn) {
	__myos_if_freenameindex(ifn);
}

// ---- event counters, timers, wait sets, signal fds ----------------------

int Sysdeps<EventfdCreate>::operator()(unsigned int initval, int flags, int *fd) {
	int r = __myos_eventfd(initval, flags);
	if (r < 0)
		return errno;
	*fd = r;
	return 0;
}

int Sysdeps<TimerfdCreate>::operator()(int clockid, int flags, int *fd) {
	int r = __myos_timerfd_create(clockid, flags);
	if (r < 0)
		return errno;
	*fd = r;
	return 0;
}

int Sysdeps<TimerfdSettime>::operator()(int fd, int flags, const struct itimerspec *value, struct itimerspec *oldvalue) {
	return err(__myos_timerfd_settime(fd, flags, value, oldvalue));
}

int Sysdeps<TimerfdGettime>::operator()(int fd, struct itimerspec *its) {
	return err(__myos_timerfd_gettime(fd, its));
}

int Sysdeps<EpollCreate>::operator()(int flags, int *fd) {
	int r = __myos_epoll_create1(flags);
	if (r < 0)
		return errno;
	*fd = r;
	return 0;
}

int Sysdeps<EpollCtl>::operator()(int epfd, int mode, int fd, struct epoll_event *ev) {
	return err(__myos_epoll_ctl(epfd, mode, fd, ev));
}

int Sysdeps<EpollPwait>::operator()(int epfd, struct epoll_event *ev, int n, int timeout, const sigset_t *sigmask, int *raised) {
	int r = __myos_epoll_pwait(epfd, ev, n, timeout, sigmask);
	if (r < 0)
		return errno;
	*raised = r;
	return 0;
}

int Sysdeps<SignalfdCreate>::operator()(const sigset_t *mask, int flags, int *fd) {
	int r = __myos_signalfd(-1, mask, flags);
	if (r < 0)
		return errno;
	*fd = r;
	return 0;
}

// ---- shared memory ------------------------------------------------------

int Sysdeps<MemfdCreate>::operator()(const char *name, int flags, int *fd) {
	int r = __myos_memfd_create(name, (unsigned)flags);
	if (r < 0)
		return errno;
	*fd = r;
	return 0;
}

int Sysdeps<Shmget>::operator()(int *shm_id, key_t key, size_t size, int shmflg) {
	int r = __myos_shmget(key, size, shmflg);
	if (r < 0)
		return errno;
	*shm_id = r;
	return 0;
}

int Sysdeps<Shmat>::operator()(void **seg_start, int shmid, const void *shmaddr, int shmflg) {
	void *p = __myos_shmat(shmid, shmaddr, shmflg);
	if (p == (void *)-1)
		return errno;
	*seg_start = p;
	return 0;
}

int Sysdeps<Shmdt>::operator()(const void *shmaddr) {
	return err(__myos_shmdt(shmaddr));
}

int Sysdeps<Shmctl>::operator()(int *idx, int shmid, int cmd, struct shmid_ds *buf) {
	int r = __myos_shmctl(shmid, cmd, buf);
	if (r < 0)
		return errno;
	*idx = r;
	return 0;
}

} // namespace mlibc
