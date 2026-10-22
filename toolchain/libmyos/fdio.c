/* The fd calls over the kernel's: read, write, close and their kin, and
 * the process calls newlib's libc reaches through its _NAME hooks (fork,
 * wait, kill, exit, getpid, sbrk, gettimeofday: no alias here, hooks.c
 * defines those). The userspace socket library (socket.c) hooks into read,
 * write, close and fcntl through the weak stubs below, so a program that
 * never pulls socket.o still links. */

#include <errno.h>
#include <fcntl.h>
#include <poll.h>
#include <stddef.h>
#include <stdint.h>
#include <string.h>
#include <sys/stat.h>
#include <sys/time.h>
#include <sys/types.h>
#include <sys/wait.h>
#include <unistd.h>

#include "myos_syscalls.h"
#include "libc.h"

static void myos_set_errno_io(void) {
    errno = EIO;
}

/* Whether `fd` is open on a terminal: what its /proc/self/fd link names
 * (ttyctl.c). errno is left alone. */
int myos_fd_is_tty(int fd) {
    int saved = errno;
    int tty = myos_tty_dir(fd, NULL, 0, NULL) == 0;
    errno = saved;
    return tty;
}

/* Hangup /net conversations tracked by userspace BSD sockets (socket.c).
 * Weak stub so programs that never pull socket.o still link. */
void myos_socket_on_close(int fd) __attribute__((weak));
void myos_socket_on_close(int fd) { (void)fd; }

/* Empty /net data read: 0=not socket, 1=EAGAIN, 2=hangup EOF, 3=retry. */

int myos_socket_empty_read(int fd) __attribute__((weak));
int myos_socket_empty_read(int fd) { (void)fd; return 0; }

int myos_socket_write_failed(int fd) __attribute__((weak));
int myos_socket_write_failed(int fd) { (void)fd; return 0; }

/* A datagram socket's read(2) / write(2), whole (socket.c): its datagrams
 * and errors are the library's. -2 = not a datagram socket. */
ssize_t myos_socket_dgram_read(int fd, void *buf, size_t cnt) __attribute__((weak));
ssize_t myos_socket_dgram_read(int fd, void *buf, size_t cnt) {
    (void)fd; (void)buf; (void)cnt;
    return -2;
}
ssize_t myos_socket_dgram_write(int fd, const void *buf, size_t cnt) __attribute__((weak));
ssize_t myos_socket_dgram_write(int fd, const void *buf, size_t cnt) {
    (void)fd; (void)buf; (void)cnt;
    return -2;
}

int myos_socket_fcntl(int fd, int cmd, int arg) __attribute__((weak));
int myos_socket_fcntl(int fd, int cmd, int arg) {
    (void)fd; (void)cmd; (void)arg;
    return -1;
}

int myos_socket_poll_prepare(int fd, short events, short *now, short *kevents)
    __attribute__((weak));
int myos_socket_poll_prepare(int fd, short events, short *now, short *kevents) {
    (void)fd; (void)events; (void)now; (void)kevents;
    return -1;
}

int myos_socket_write_all(int fd) __attribute__((weak));
int myos_socket_write_all(int fd) {
    (void)fd;
    return 0;
}

void myos_socket_poll_done(int fd, short events, short *revents) __attribute__((weak));
void myos_socket_poll_done(int fd, short events, short *revents) {
    (void)fd; (void)events; (void)revents;
}

/* The cancellation points and the fork hooks are the threads library's
 * (newlib: libgloss/myos/pthread.c; mlibc keeps its own cancellation and
 * atfork handling above the sysdeps): nothing to do without one. */
void __myos_cancel_enter(void) __attribute__((weak));
void __myos_cancel_enter(void) {}
void __myos_cancel_leave(void) __attribute__((weak));
void __myos_cancel_leave(void) {}
void __myos_fork_prepare(void) __attribute__((weak));
void __myos_fork_prepare(void) {}
void __myos_fork_done(int child) __attribute__((weak));
void __myos_fork_done(int child) { (void)child; }

/* SYS_POLL. Here rather than in pollselect.c: read uses it, and linking
 * pollselect.o into every program would clash with a port's own select()
 * (vim's myos_stubs.c). */
int __myos_kpoll(struct pollfd *fds, unsigned long nfds, int timeout) {
    long ret = myos_syscall3(MYOS_SYS_POLL, (long)(uintptr_t)fds, (long)nfds, (long)timeout);
    if (ret == (long)MYOS_EINTR) {
        errno = EINTR;
        return -1;
    }
    if (ret == (long)MYOS_SYSERR) {
        errno = EINVAL;
        return -1;
    }
    return (int)ret;
}

/* O_NONBLOCK is userspace-tracked: kernel reads always block. Dropbear
 * setnonblocking(signal_pipe) then drains with `while (read > 0)` — without
 * EAGAIN on empty, a forced-POLLIN wake hangs the session forever and the SSH
 * client never receives exit-status. */
static unsigned long long myos_fd_nb_mask[MYOS_MAX_FDS / 64];

void myos_fd_nonblock_set(int fd, int on) {
    if (fd < 0 || fd >= MYOS_MAX_FDS) {
        return;
    }
    if (on) {
        myos_fd_nb_mask[fd / 64] |= 1ull << (fd % 64);
    } else {
        myos_fd_nb_mask[fd / 64] &= ~(1ull << (fd % 64));
    }
}

int myos_fd_nonblock_get(int fd) {
    return (fd >= 0 && fd < MYOS_MAX_FDS && (myos_fd_nb_mask[fd / 64] & (1ull << (fd % 64)))) ? 1 : 0;
}

void myos_fd_nonblock_clear(int fd) {
    myos_fd_nonblock_set(fd, 0);
}

void myos_fd_nonblock_dup(int from, int to) {
    myos_fd_nonblock_set(to, myos_fd_nonblock_get(from));
}

/* A directory listing kept per fd (mlibc's sysdeps) goes with the fd. */
void __myos_dir_closed(int fd) __attribute__((weak));
void __myos_dir_closed(int fd) { (void)fd; }
/* The fds evfd.c knows as signal fds too. */
void myos_evfd_on_close(int fd) __attribute__((weak));
void myos_evfd_on_close(int fd) { (void)fd; }

/* A signal fd's read when the libc's struct signalfd_siginfo is not the
 * kernel's 16-byte record (evfd.c): how many bytes of records the
 * structs `cnt` holds take (1), or 0 for any other fd or a libc whose
 * struct is the record, or -1 with errno; and the records read widened
 * into the structs, in place. */
int myos_sigfd_kernel_count(int fd, size_t cnt, size_t *kcnt) __attribute__((weak));
int myos_sigfd_kernel_count(int fd, size_t cnt, size_t *kcnt) {
    (void)fd;
    (void)cnt;
    (void)kcnt;
    return 0;
}
ssize_t myos_sigfd_expand(void *buf, ssize_t n) __attribute__((weak));
ssize_t myos_sigfd_expand(void *buf, ssize_t n) {
    (void)buf;
    return n;
}

int __myos_close(int fd) {
    myos_socket_on_close(fd);
    __myos_dir_closed(fd);
    myos_evfd_on_close(fd);
    long ret = myos_syscall1(MYOS_SYS_CLOSE, fd);
    if (ret == (long)MYOS_SYSERR) {
        errno = EBADF;
        return -1;
    }
    myos_fd_nonblock_clear(fd);
    return 0;
}

void __myos_exit(int status) {
    myos_syscall1(MYOS_SYS_EXIT, status);
    for (;;) {
    }
}

/* read(2) and pread(2): MYOS_SYS_PREAD at the file position (flags 0) or at
 * `off` (MYOS_FILE_AT). */
static ssize_t read_at(int fd, void *buf, size_t cnt, off_t off, long flags) {
    if (flags == 0) {
        ssize_t n = myos_socket_dgram_read(fd, buf, cnt);
        if (n != -2) {
            return n;
        }
    }

    /* Honour O_NONBLOCK before the blocking read (dropbear's signal-pipe
     * drain must not hang): ask the kernel whether the read would block. */
    if (myos_fd_nonblock_get(fd)) {
        struct pollfd p = {fd, POLLIN, 0};
        if (__myos_kpoll(&p, 1, 0) == 0) {
            errno = EAGAIN;
            return -1;
        }
    }

    size_t kcnt = cnt;
    int sigfd = myos_sigfd_kernel_count(fd, cnt, &kcnt);
    if (sigfd < 0) {
        return -1;
    }
    for (;;) {
        long ret = myos_syscall6(MYOS_SYS_PREAD, fd, (long)(uintptr_t)buf, (long)kcnt, (long)off, flags, 0);
        if (ret == (long)MYOS_ESPIPE) {
            errno = ESPIPE;
            return -1;
        }
        if (ret == (long)MYOS_EINTR) {
            errno = EINTR; /* a caught signal interrupted a blocked read */
            return -1;
        }
        if (ret == (long)MYOS_EIO) {
            errno = EIO; /* pty peer gone */
            return -1;
        }
        if (ret == (long)MYOS_SYSERR) {
            errno = EBADF;
            return -1;
        }
        /* /net data returns 0 when empty. Nonblock -> EAGAIN; blocking waits. */
        if (ret == 0) {
            int kind = myos_socket_empty_read(fd);
            if (kind == 1) {
                errno = EAGAIN;
                return -1;
            }
            if (kind == 2) {
                return 0; /* hangup EOF */
            }
            if (kind == 3) {
                continue; /* blocking wait finished; retry syscall */
            }
            if (kind == 4) {
                errno = EINTR; /* a caught signal ended the wait */
                return -1;
            }
        }
        return sigfd ? myos_sigfd_expand(buf, (ssize_t)ret) : (ssize_t)ret;
    }
}

ssize_t __myos_read(int fd, void *buf, size_t cnt) {
    __myos_cancel_enter();
    ssize_t n = read_at(fd, buf, cnt, 0, 0);
    __myos_cancel_leave();
    return n;
}

ssize_t __myos_pread(int fd, void *buf, size_t cnt, off_t off) {
    if (off < 0) {
        errno = EINVAL;
        return -1;
    }
    __myos_cancel_enter();
    ssize_t n = read_at(fd, buf, cnt, off, MYOS_FILE_AT);
    __myos_cancel_leave();
    return n;
}

/* write(2) and pwrite(2), as read_at. */
static ssize_t write_at(int fd, const void *buf, size_t cnt, off_t off, long flags) {
    long ret;
    size_t done = 0;

    if (flags == 0) {
        ssize_t n = myos_socket_dgram_write(fd, buf, cnt);
        if (n != -2) {
            return n;
        }
    }

    for (;;) {
        ret = myos_syscall6(MYOS_SYS_PWRITE, fd, (long)(uintptr_t)buf + done, (long)(cnt - done),
                            (long)(off + (off_t)done), flags, 0);
        if (ret == (long)MYOS_ESPIPE) {
            errno = ESPIPE;
            return -1;
        }
        if (ret == (long)MYOS_EINTR) {
            errno = EINTR; /* a caught signal interrupted a blocked write */
            return done ? (ssize_t)done : -1;
        }
        if (ret == (long)MYOS_EIO) {
            errno = EIO; /* pty peer gone */
            return done ? (ssize_t)done : -1;
        }
        if (ret == (long)MYOS_ENOSPC) {
            errno = ENOSPC; /* the filesystem is full */
            return done ? (ssize_t)done : -1;
        }
        if (ret == (long)MYOS_SYSERR) {
            /* A stream socket refuses a write while it has no room. */
            switch (myos_socket_write_failed(fd)) {
            case 1:
                errno = EAGAIN;
                return done ? (ssize_t)done : -1;
            case 2:
                errno = EPIPE;
                return done ? (ssize_t)done : -1;
            case 3:
                continue; /* waited for room; retry */
            case 4:
                errno = ENOTCONN;
                return done ? (ssize_t)done : -1;
            case 5:
                errno = EINTR; /* a caught signal ended the wait */
                return done ? (ssize_t)done : -1;
            }
            myos_set_errno_io();
            return done ? (ssize_t)done : -1;
        }
        done += (size_t)ret;
        /* A blocking stream socket took part of it (no more room): write
         * the rest, waiting for room as above. */
        if (ret > 0 && done < cnt && myos_socket_write_all(fd)) {
            continue;
        }
        return (ssize_t)done;
    }
}

ssize_t __myos_write(int fd, const void *buf, size_t cnt) {
    __myos_cancel_enter();
    ssize_t n = write_at(fd, buf, cnt, 0, 0);
    __myos_cancel_leave();
    return n;
}

ssize_t __myos_pwrite(int fd, const void *buf, size_t cnt, off_t off) {
    if (off < 0) {
        errno = EINVAL;
        return -1;
    }
    __myos_cancel_enter();
    ssize_t n = write_at(fd, buf, cnt, off, MYOS_FILE_AT);
    __myos_cancel_leave();
    return n;
}

off_t __myos_lseek(int fd, off_t pos, int whence) {
    long ret = myos_syscall3(MYOS_SYS_LSEEK, fd, (long)pos, whence);
    if (ret == (long)MYOS_SYSERR) {
        errno = ESPIPE;
        return -1;
    }
    return (off_t)ret;
}

/* The break is read anew each time, not kept: a Rust program's allocator
 * (user/lib) moves the same break when a C library is linked in (user/tls),
 * and each side only ever extends it from where it is. */
void *__myos_sbrk(ptrdiff_t incr) {
    void *cur = (void *)(uintptr_t)myos_syscall1(MYOS_SYS_BRK, 0);
    void *next = (char *)cur + incr;
    void *got = (void *)(uintptr_t)myos_syscall1(MYOS_SYS_BRK, (long)(uintptr_t)next);
    if (got != next) {
        errno = ENOMEM;
        return (void *)-1;
    }
    return cur;
}

int __myos_getpid(void) {
    long ret = myos_syscall0(MYOS_SYS_GETPID);
    if (ret == (long)MYOS_SYSERR) {
        return 1;
    }
    return (int)ret;
}

int __myos_kill(int pid, int sig) {
    /* Signal numbers must match <signal.h> / kernel signal.rs; 0 only
     * checks that the target exists. */
    if (sig < 0 || sig > 31) {
        errno = EINVAL;
        return -1;
    }
    long ret = myos_syscall3(MYOS_SYS_KILL, (long)pid, (long)sig, 0);
    if (ret == (long)MYOS_SYSERR) {
        errno = ESRCH;
        return -1;
    }
    return 0;
}

int __myos_fork(void) {
    __myos_fork_prepare();
    long ret = myos_syscall0(MYOS_SYS_FORK);
    __myos_fork_done(ret == 0);
    if (ret == (long)MYOS_SYSERR) {
        errno = EAGAIN;
        return -1;
    }
    return (int)ret;
}

#ifndef WNOHANG
#define WNOHANG 1
#endif

/*
 * SYS_WAITPID(status, options, pid): the kernel writes a POSIX int status
 * (WIFSIGNALED for a signal death) and waits for `pid` when it is > 0, for
 * any child otherwise (process-group waits are treated as "any"). WNOHANG
 * keeps dropbear's `while (waitpid(-1, &st, WNOHANG) > 0)` reap loop from
 * blocking after the last zombie. WUNTRACED is ignored: nothing stops.
 */
static pid_t waitpid_body(pid_t pid, int *status, int options) {
    int st = 0;
    long ret;
    long opts = 0;

    if ((options & WNOHANG) != 0) {
        opts |= MYOS_WAIT_NOHANG;
    }

    ret = myos_syscall3(MYOS_SYS_WAITPID, status ? (long)(uintptr_t)&st : 0, opts,
                        pid > 0 ? (long)pid : 0);

    if (ret == (long)MYOS_EINTR) {
        errno = EINTR;
        return -1;
    }
    if (ret == (long)MYOS_SYSERR) {
        errno = ECHILD;
        return -1;
    }
    /* Kernel returns 0 for WNOHANG with live children but no zombie. */
    if (ret == 0 && (options & WNOHANG) != 0) {
        return 0;
    }
    if (status != NULL) {
        *status = st;
    }
    return (pid_t)ret;
}

/* A cancellation point. */
pid_t __myos_waitpid(pid_t pid, int *status, int options) {
    __myos_cancel_enter();
    pid_t r = waitpid_body(pid, status, options);
    __myos_cancel_leave();
    return r;
}

pid_t __myos_wait(int *status) {
    return __myos_waitpid(-1, status, 0);
}

int __myos_gettimeofday(struct timeval *tv, void *tz) {
    (void)tz;
    if (tv == NULL) {
        errno = EINVAL;
        return -1;
    }
    long ret = myos_syscall3(
        MYOS_SYS_GETTIMEOFDAY, (long)(uintptr_t)tv, 0, 0);
    if (ret == (long)MYOS_SYSERR) {
        errno = EIO;
        return -1;
    }
    return 0;
}

MYOS_ALIAS(pread)
MYOS_ALIAS(pwrite)
MYOS_ALIAS(waitpid)
