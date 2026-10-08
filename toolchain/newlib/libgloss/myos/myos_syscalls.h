#ifndef MYOS_SYSCALLS_H
#define MYOS_SYSCALLS_H

#include <stddef.h>

struct termios;
struct winsize;

/* Per-process fd table size (kernel/src/task/mod.rs MAX_FDS). */
#define MYOS_MAX_FDS 64

#define MYOS_SYS_EXIT 1
#define MYOS_SYS_CLOSE 4
#define MYOS_SYS_FORK 6
#define MYOS_SYS_WAIT 7
#define MYOS_SYS_BRK 9
/* pipe(fds, flags), dupfd(fd, min, flags): MYOS_FD_CLOEXEC in flags makes
 * the new fds close at exec. */
#define MYOS_SYS_PIPE 10
#define MYOS_SYS_DUP2 11
#define MYOS_SYS_EXECNAME 13
#define MYOS_SYS_DUPFD 14
#define MYOS_SYS_GETCWD 16
#define MYOS_SYS_MMAP 23
#define MYOS_SYS_MUNMAP 24
#define MYOS_SYS_MPROTECT 25
#define MYOS_SYS_LSEEK 26
#define MYOS_SYS_MOUNT 27
#define MYOS_SYS_SETSID 29
#define MYOS_SYS_SETPGID 30
#define MYOS_SYS_GETPGID 31
#define MYOS_SYS_GETSID 32
#define MYOS_SYS_GETTIMEOFDAY 33
#define MYOS_SYS_KILL 34
#define MYOS_SYS_SIGACTION 35
#define MYOS_SYS_GETPID 36
#define MYOS_SYS_SIGPROCMASK 37
/* poll(fds, nfds, timeout_ms): the kernel scans struct pollfd and sleeps until
 * the first one is ready (pollselect.c). */
#define MYOS_SYS_POLL 38
#define MYOS_SYS_SIGCHLD_TAKE 39
#define MYOS_SYS_SIGCHLD_PENDING 41
#define MYOS_SYS_PIPE_PEER 42
/* Return from a signal handler: the trampoline is done with the frame at the
 * stack pointer (signal.c, kernel/src/signal.rs). */
#define MYOS_SYS_SIGRETURN 45

/* waitpid(status, options, pid): POSIX int status (WIFSIGNALED-aware),
 * pid > 0 waits for that child. SYS_WAIT keeps the legacy exit-code byte. */
#define MYOS_SYS_WAITPID 46
#define MYOS_SYS_SIGPENDING 47
#define MYOS_SYS_SIGSUSPEND 48
#define MYOS_SYS_SIGWAIT 49
/* sigaction with the trampoline as a 4th struct word (signal.c). */
#define MYOS_SYS_SIGACTION2 50
/* nanosleep(ns, flags): the task blocks (its CPU halts) until the deadline or
 * a signal (EINTR). MYOS_SLEEP_ANY_EVENT also ends the sleep on any kernel
 * event a poller cares about (console/pipe/pty/device traffic, an exit). */
#define MYOS_SYS_NANOSLEEP 52
#define MYOS_SLEEP_ANY_EVENT 1
#define MYOS_SYS_GETPPID 60
/* settimeofday(tv): two int64_t, seconds and microseconds (time.c). */
#define MYOS_SYS_SETTIMEOFDAY 65
/* setuser(buf, len): "name\0password"; ns(spec, len); policy_load(path,
 * len) (docs/security.md, pwdgrp.c). */
#define MYOS_SYS_SETUSER 67
#define MYOS_SYS_NS 68
#define MYOS_SYS_POLICY_LOAD 69
/* The path calls (at.c, kernel/src/user/at.rs): a directory fd
 * (MYOS_AT_FDCWD: the cwd) and a path (pointer, length) relative to it;
 * MYOS_AT_EMPTY_PATH with an empty path is the fd's own file.
 *   openat(dirfd, path, len, flags)           an fd
 *   statat(dirfd, path, len, flags, out)      struct myos_stat (myos_stat.h)
 *   mknodat(dirfd, path, len, kind)           MYOS_MKNOD_DIR or _FIFO
 *   symlinkat(target, tlen, dirfd, path, len)
 *   unlinkat(dirfd, path, len, flags)         MYOS_AT_REMOVEDIR: a directory
 *   renameat(odirfd, old, olen, ndirfd, new, nlen)
 *   readlinkat(dirfd, path, len, buf, size)   the target's length
 *   utimensat(dirfd, path, len, times, flags) times: two int64_t seconds,
 *                                             MYOS_UTIME_NOW or _OMIT; NULL:
 *                                             both now
 *   chdirat(dirfd, path, len, flags)
 *   listdirat(dirfd, path, len, buf, cap, flags)  names, one per line
 *   execat(dirfd, path, len, pack, flags)     the exec block (_execve) */
#define MYOS_SYS_OPENAT 70
#define MYOS_SYS_STATAT 71
#define MYOS_SYS_MKNODAT 72
#define MYOS_SYS_SYMLINKAT 73
#define MYOS_SYS_UNLINKAT 74
#define MYOS_SYS_RENAMEAT 75
#define MYOS_SYS_READLINKAT 76
#define MYOS_SYS_UTIMENSAT 77
#define MYOS_SYS_CHDIRAT 78
#define MYOS_SYS_LISTDIRAT 79
#define MYOS_SYS_EXECAT 80
#define MYOS_AT_FDCWD (-100L)
#define MYOS_AT_SYMLINK_NOFOLLOW 0x100
#define MYOS_AT_REMOVEDIR 0x200
#define MYOS_AT_EMPTY_PATH 0x1000
/* pread(fd, buf, len, offset, flags) and pwrite(...): at the file position,
 * which advances, or with MYOS_FILE_AT at `offset`, the position left as it
 * is (MYOS_ESPIPE on a pipe or terminal). They are read and write too.
 * ftruncate(fd, size): cut or grow (with zeros) a file open for writing.
 * fdflags(fd, op, flags): MYOS_FD_GET returns the fd's flags, MYOS_FD_SET
 * replaces them; the one flag is MYOS_FD_CLOEXEC (the fd closes at exec). */
#define MYOS_SYS_PREAD 81
#define MYOS_SYS_PWRITE 82
#define MYOS_SYS_FTRUNCATE 83
#define MYOS_SYS_FDFLAGS 84
#define MYOS_FILE_AT 1
#define MYOS_FD_GET 0
#define MYOS_FD_SET 1
#define MYOS_FD_CLOEXEC 1
/* flock(fd, op): op is LOCK_SH, LOCK_EX or LOCK_UN, | LOCK_NB not to wait
 * (the values of <sys/file.h>). lockctl(fd, cmd, struct myos_lock_range *):
 * a record lock, MYOS_LOCKCTL_GET (the range is filled with the first
 * conflicting lock, or its kind set to MYOS_LOCK_UNLOCK), _SET or _WAIT,
 * | MYOS_LOCKCTL_OFD for a lock of the open file description rather than
 * the process. MYOS_EAGAIN: someone else holds a conflicting lock. */
#define MYOS_SYS_FLOCK 85
#define MYOS_SYS_LOCKCTL 86
/* power(action): power off, reboot or halt, after the processes are
 * stopped and the disks unmounted (docs/power.md). Returns only on
 * failure (no `write` on kernel.power). */
#define MYOS_SYS_POWER 87
/* itimer(which, new, old): ITIMER_REAL only (SIGALRM); new and old point
 * at two u64, the microseconds until it fires (0: disarmed) and its
 * interval (0: once). Either may be 0. */
#define MYOS_SYS_ITIMER 88
/* msync(addr, len, flags): the shared file mappings in the range are
 * written back to their files (the flags make no difference). */
#define MYOS_SYS_MSYNC 92
/* clock_monotonic(): nanoseconds since boot (clock_gettime's CLOCK_MONOTONIC). */
#define MYOS_SYS_CLOCK_MONOTONIC 91
#define MYOS_POWER_OFF 0
#define MYOS_POWER_REBOOT 1
#define MYOS_POWER_HALT 2
#define MYOS_LOCKCTL_GET 0
#define MYOS_LOCKCTL_SET 1
#define MYOS_LOCKCTL_WAIT 2
#define MYOS_LOCKCTL_OFD 0x10
#define MYOS_LOCK_SHARED 0
#define MYOS_LOCK_EXCLUSIVE 1
#define MYOS_LOCK_UNLOCK 2
struct myos_lock_range {
    unsigned int kind;
    unsigned int pad;
    unsigned long long start;
    unsigned long long len; /* 0: to the end of the file, however long */
    long long pid;          /* GET: the holder, -1 for a description's lock */
};
#define MYOS_MKNOD_DIR 0
#define MYOS_MKNOD_FIFO 1
#define MYOS_UTIME_NOW (-1LL)
#define MYOS_UTIME_OMIT (-2LL)

/* fds per process (kernel MAX_FDS): sysconf(_SC_OPEN_MAX), getdtablesize. */
#define MYOS_OPEN_MAX 64
#define MYOS_WAIT_NOHANG 1

#define MYOS_STR_(x) #x
#define MYOS_STR(x) MYOS_STR_(x)

#define MYOS_SYSERR ((unsigned long)-1)
/* Distinct pty peer-gone error (kernel/src/task/mod.rs SYSERR_EIO): read or
 * write on a hung-up pty end. Mapped to errno = EIO by read/write wrappers. */
#define MYOS_EIO ((unsigned long)-2)
/* open() of a FIFO for writing with O_NONBLOCK and no reader. */
#define MYOS_ENXIO ((unsigned long)-3)
/* A blocking syscall interrupted by a caught signal (kernel SYSERR_EINTR). */
#define MYOS_EINTR ((unsigned long)-4)
/* An O_CREAT|O_EXCL open of a name that is taken. */
#define MYOS_EEXIST ((unsigned long)-5)
/* A read or write at an offset on a pipe or terminal. */
#define MYOS_ESPIPE ((unsigned long)-6)
/* An O_NOFOLLOW open of a symlink; an O_DIRECTORY open of something else. */
#define MYOS_ELOOP ((unsigned long)-7)
#define MYOS_ENOTDIR ((unsigned long)-8)
/* A lock someone else holds (flock and lockctl without waiting). */
#define MYOS_EAGAIN ((unsigned long)-9)
/* setpgid of a child that has exec'd. */
#define MYOS_EACCES ((unsigned long)-10)

/* Sleep `ns` nanoseconds (sleep.c). 0 = slept (or an event with
 * MYOS_SLEEP_ANY_EVENT); -1 with errno = EINTR when a caught signal ran. */
int __myos_sleep_ns(unsigned long long ns, int flags);

long myos_syscall0(long nr);
long myos_syscall1(long nr, long a0);
long myos_syscall2(long nr, long a0, long a1);
long myos_syscall3(long nr, long a0, long a1, long a2);
long myos_syscall6(long nr, long a0, long a1, long a2, long a3, long a4, long a5);

/* The names in a directory, one per line, into buf: `path` relative to
 * `dirfd` (at.c's dirfd and flag values, AT_EMPTY_PATH for the fd's own);
 * the bytes written, -1 with errno set. */
/* pipe(2) with both ends closing at exec when `cloexec` (pipe2). */
int myos_pipe_flags(int fildes[2], int cloexec);
long myos_listdirat(int dirfd, const char *path, char *buf, size_t cap, int flags);

/* The terminal behind an fd, through its files (ttyctl.c, docs/tty.md). */
#define MYOS_TTY_PATH 64
#define MYOS_TTY_CTL 256
int myos_fd_is_tty(int fd);
/* The directory of the terminal `fd` is open on (/dev/console, /dev/pts/N)
 * into `dir`, `*master` set when the fd is a pty's master end; -1 with
 * ENOTTY when it is not a terminal. `dir` and `master` may be NULL. */
int myos_tty_dir(int fd, char *dir, size_t cap, int *master);
/* Read the terminal's termios and/or window size (either may be NULL). */
int myos_tty_get(int fd, struct termios *t, struct winsize *w);
/* Write the termios, or the window size, to the terminal. */
int myos_tty_set(int fd, const struct termios *t);
int myos_tty_set_winsize(int fd, unsigned rows, unsigned cols);
/* Write lines to the terminal's ctl (`ctty`, `flush`, `winsize`). */
int myos_tty_write(int fd, const char *text);


/* Userspace BSD sockets (socket.c); weak stubs in syscalls.c. */
void myos_socket_on_close(int fd);
int myos_socket_empty_read(int fd);
int myos_socket_fcntl(int fd, int cmd, int arg);
int myos_socket_write_failed(int fd);
int myos_socket_write_all(int fd);
/* poll() around the kernel call (pollselect.c): before it, a tracked socket
 * sets what is ready already (*now) and what the kernel should wait for
 * (*kevents), returning 0 (-1: not a socket); after it, done() turns the
 * kernel's revents into the socket's. */
int myos_socket_poll_prepare(int fd, short events, short *now, short *kevents);
void myos_socket_poll_done(int fd, short events, short *revents);

/* SYS_POLL (syscalls.c): count of ready fds, or -1 with errno (EINTR). */
struct pollfd;
int __myos_kpoll(struct pollfd *fds, unsigned long nfds, int timeout);


/* Userspace O_NONBLOCK tracking (kernel reads always block): a nonblocking
 * read first asks the kernel whether it would. */
void myos_fd_nonblock_set(int fd, int on);
int myos_fd_nonblock_get(int fd);
void myos_fd_nonblock_clear(int fd);
void myos_fd_nonblock_dup(int from, int to);

#endif

