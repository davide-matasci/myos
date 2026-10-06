/* myos libgloss: map newlib hooks to the existing myos syscall ABI. */

#include <_ansi.h>
#include <errno.h>
#include <fcntl.h>
#include <poll.h>
#include <reent.h>
#include <stddef.h>
#include <string.h>
#include <sys/stat.h>
#include <sys/types.h>
#include <unistd.h>

#include "myos_syscalls.h"

static int myos_err(long ret) {
    if (ret == (long)MYOS_SYSERR) {
        return -1;
    }
    return (int)ret;
}

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


/* SYS_POLL. Here rather than in pollselect.c: _read uses it, and linking
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
static unsigned long long myos_fd_nb_mask;

void myos_fd_nonblock_set(int fd, int on) {
    if (fd < 0 || fd >= MYOS_MAX_FDS) {
        return;
    }
    if (on) {
        myos_fd_nb_mask |= (1ull << fd);
    } else {
        myos_fd_nb_mask &= ~(1ull << fd);
    }
}

int myos_fd_nonblock_get(int fd) {
    return (fd >= 0 && fd < MYOS_MAX_FDS && (myos_fd_nb_mask & (1ull << fd))) ? 1 : 0;
}

void myos_fd_nonblock_clear(int fd) {
    myos_fd_nonblock_set(fd, 0);
}

void myos_fd_nonblock_dup(int from, int to) {
    myos_fd_nonblock_set(to, myos_fd_nonblock_get(from));
}

int _close(int fd) {

    myos_socket_on_close(fd);
    long ret = myos_syscall1(MYOS_SYS_CLOSE, fd);
    if (ret == (long)MYOS_SYSERR) {
        errno = EBADF;
        return -1;
    }
    myos_fd_nonblock_clear(fd);
    return 0;
}

void _exit(int status) {

    myos_syscall1(MYOS_SYS_EXIT, status);
    for (;;) {
    }
}

/*
 * Kernel SYS_OPEN flags are Linux-shaped (O_CREAT 0100, O_TRUNC 01000,
 * O_APPEND 02000). newlib <fcntl.h> is BSD-shaped (O_CREAT 0x0200,
 * O_TRUNC 0x0400, O_APPEND 0x0008). O_ACCMODE already matches.
 *
 * Passing newlib bits through made `echo > /tmp/file` fail: the kernel never
 * saw O_CREAT (and treated 0x0200 as O_TRUNC). Map here; this is syscall glue.
 */
#define MYOS_K_O_CREAT  0x40
#define MYOS_K_O_TRUNC  0x200
#define MYOS_K_O_APPEND 0x400
#define MYOS_K_O_NONBLOCK 0x800


/* read(2) and pread(2): MYOS_SYS_PREAD at the file position (flags 0) or at
 * `off` (MYOS_FILE_AT). */
static ssize_t read_at(int fd, void *buf, size_t cnt, off_t off, long flags) {

    /* Honour O_NONBLOCK before the blocking read (dropbear's signal-pipe
     * drain must not hang): ask the kernel whether the read would block. */
    if (myos_fd_nonblock_get(fd)) {
        struct pollfd p = {fd, POLLIN, 0};
        if (__myos_kpoll(&p, 1, 0) == 0) {
            errno = EAGAIN;
            return -1;
        }
    }

    for (;;) {
        long ret = myos_syscall6(MYOS_SYS_PREAD, fd, (long)(uintptr_t)buf, (long)cnt, (long)off, flags, 0);
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
        return (ssize_t)ret;
    }
}

int _read(int fd, void *buf, size_t cnt) {
    return (int)read_at(fd, buf, cnt, 0, 0);
}

ssize_t pread(int fd, void *buf, size_t cnt, off_t off) {
    if (off < 0) {
        errno = EINVAL;
        return -1;
    }
    return read_at(fd, buf, cnt, off, MYOS_FILE_AT);
}

/* write(2) and pwrite(2), as read_at. */
static ssize_t write_at(int fd, const void *buf, size_t cnt, off_t off, long flags) {
    long ret;
    size_t done = 0;

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

int _write(int fd, const void *buf, size_t cnt) {
    return (int)write_at(fd, buf, cnt, 0, 0);
}

ssize_t pwrite(int fd, const void *buf, size_t cnt, off_t off) {
    if (off < 0) {
        errno = EINVAL;
        return -1;
    }
    return write_at(fd, buf, cnt, off, MYOS_FILE_AT);
}

int _isatty(int fd) {
    if (myos_fd_is_tty(fd)) {
        return 1;
    }
    errno = ENOTTY;
    return 0;
}

void *_sbrk(ptrdiff_t incr) {
    static void *cur;
    if (cur == NULL) {
        cur = (void *)(uintptr_t)myos_syscall1(MYOS_SYS_BRK, 0);
    }
    void *old = cur;
    void *next = (char *)cur + incr;
    void *got = (void *)(uintptr_t)myos_syscall1(MYOS_SYS_BRK, (long)(uintptr_t)next);
    if (got != next) {
        errno = ENOMEM;
        return (void *)-1;
    }
    cur = next;
    return old;
}

int _getpid(void) {
    long ret = myos_syscall0(MYOS_SYS_GETPID);
    if (ret == (long)MYOS_SYSERR) {
        return 1;
    }
    return (int)ret;
}
