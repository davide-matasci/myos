#ifndef MYOS_SYSCALLS_H
#define MYOS_SYSCALLS_H

#include <stddef.h>

struct termios;
struct winsize;

/* Per-process fd table size (kernel/src/task/mod.rs MAX_FDS). */
#define MYOS_MAX_FDS 64

#define MYOS_SYS_WRITE 0
#define MYOS_SYS_EXIT 1
#define MYOS_SYS_OPEN 2
#define MYOS_SYS_READ 3
#define MYOS_SYS_CLOSE 4
#define MYOS_SYS_EXEC 5
#define MYOS_SYS_FORK 6
#define MYOS_SYS_WAIT 7
#define MYOS_SYS_LISTDIR 8
#define MYOS_SYS_BRK 9
#define MYOS_SYS_PIPE 10
#define MYOS_SYS_DUP2 11
#define MYOS_SYS_STAT 12
#define MYOS_SYS_EXECNAME 13
#define MYOS_SYS_DUPFD 14
#define MYOS_SYS_CHDIR 15
#define MYOS_SYS_GETCWD 16
#define MYOS_SYS_MKDIR 17
#define MYOS_SYS_RMDIR 18
#define MYOS_SYS_UNLINK 19
#define MYOS_SYS_RENAME 20
#define MYOS_SYS_SYMLINK 21
#define MYOS_SYS_READLINK 22
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
#define MYOS_SYS_CHROOT 43
#define MYOS_SYS_MKFIFO 44
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
/* stat with 64-bit size and the access/modification times (myos_stat.h). */
#define MYOS_SYS_STAT2 61
/* utimens(path, len, times) / futimens(fd, times): times is two int64_t
 * seconds, MYOS_UTIME_NOW or MYOS_UTIME_OMIT; NULL sets both to now. */
#define MYOS_SYS_UTIMENS 62
#define MYOS_SYS_FUTIMENS 63
#define MYOS_UTIME_NOW (-1LL)
#define MYOS_UTIME_OMIT (-2LL)
/* settimeofday(tv): two int64_t, seconds and microseconds (time.c). */
#define MYOS_SYS_SETTIMEOFDAY 65
/* stat2 and the owner's uid (myos_stat.h). */
#define MYOS_SYS_STAT3 66
/* setuser(buf, len): "name\0password"; ns(spec, len); policy_load(path,
 * len) (docs/security.md, pwdgrp.c). */
#define MYOS_SYS_SETUSER 67
#define MYOS_SYS_NS 68
#define MYOS_SYS_POLICY_LOAD 69

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

/* Sleep `ns` nanoseconds (sleep.c). 0 = slept (or an event with
 * MYOS_SLEEP_ANY_EVENT); -1 with errno = EINTR when a caught signal ran. */
int __myos_sleep_ns(unsigned long long ns, int flags);

long myos_syscall0(long nr);
long myos_syscall1(long nr, long a0);
long myos_syscall2(long nr, long a0, long a1);
long myos_syscall3(long nr, long a0, long a1, long a2);

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

void myos_fd_path_set(int fd, const char *path);
const char *myos_fd_path_get(int fd);
void myos_fd_path_clear(int fd);
void myos_fd_path_dup(int oldfd, int newfd);
int myos_fd_path_resolve(int dirfd, const char *path, char *out, size_t outsz);

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

