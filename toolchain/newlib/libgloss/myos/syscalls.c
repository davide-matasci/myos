/* myos libgloss: map newlib hooks to the existing myos syscall ABI. */

#include <_ansi.h>
#include <errno.h>
#include <fcntl.h>
#include <reent.h>
#include <stddef.h>
#include <string.h>
#include <sys/stat.h>
#include <sys/types.h>
#include <unistd.h>

#include "myos_syscalls.h"

#ifndef O_CLOFORK
/* FreeBSD / POSIX.1-2024 close-on-fork. */
#define O_CLOFORK 0x01000000
#endif

#include "myos_stat.h"

static int myos_err(long ret) {
    if (ret == (long)MYOS_SYSERR) {
        return -1;
    }
    return (int)ret;
}

static void myos_set_errno_io(void) {
    errno = EIO;
}

/* fds 0-2 start as the hardware console; open(/dev/console) and a successful
 * open(/dev/tty) (requires ctty) also mark the returned fd as a tty. */
static unsigned myos_tty_mask = 0x7;

int myos_fd_is_tty(int fd) {
    if (fd >= 0 && fd <= 2) {
        return 1;
    }
    if (fd >= 0 && fd < 32 && (myos_tty_mask & (1u << fd))) {
        return 1;
    }
    return 0;
}

void myos_fd_set_tty(int fd, int on) {
    if (fd < 0 || fd >= 32) {
        return;
    }
    if (on) {
        myos_tty_mask |= (1u << fd);
    } else {
        myos_tty_mask &= ~(1u << fd);
    }
}

void myos_fd_dup_tty(int oldfd, int newfd) {
    myos_fd_set_tty(newfd, myos_fd_is_tty(oldfd));
}

/* True if `path` names a tty device node (for isatty bookkeeping after open).
 * `/dev/console` = hardware console; `/dev/tty` = controlling tty (kernel
 * may reject open with ENXIO when the process has no ctty). */
static int myos_path_is_tty(const char *path) {
    const char *base;
    if (path == NULL) {
        return 0;
    }
    if (strcmp(path, "/dev/console") == 0 || strcmp(path, "/dev/tty") == 0) {
        return 1;
    }
    base = strrchr(path, '/');
    base = base ? base + 1 : path;
    return strcmp(base, "console") == 0
        || strcmp(base, "tty") == 0
        || strcmp(base, "tty1") == 0;
}

static int myos_path_is_dev_tty(const char *path) {
    return path != NULL && strcmp(path, "/dev/tty") == 0;
}

/* Hangup /net conversations tracked by userspace BSD sockets (socket.c).
 * Weak stub so programs that never pull socket.o still link. */
void myos_socket_on_close(int fd) __attribute__((weak));
void myos_socket_on_close(int fd) { (void)fd; }

/* Empty /net data read: 0=not socket, 1=EAGAIN, 2=hangup EOF, 3=retry. */
extern int myos_sigchld_armed(void);

int myos_socket_empty_read(int fd) __attribute__((weak));
int myos_socket_empty_read(int fd) { (void)fd; return 0; }

int myos_socket_fcntl(int fd, int cmd, int arg) __attribute__((weak));
int myos_socket_fcntl(int fd, int cmd, int arg) {
    (void)fd; (void)cmd; (void)arg;
    return -1;
}

int myos_socket_poll(int fd, short events, short *revents) __attribute__((weak));
int myos_socket_poll(int fd, short events, short *revents) {
    (void)fd; (void)events; (void)revents;
    return -1;
}


/* O_NONBLOCK is userspace-tracked for pipes: the kernel always blocks. Dropbear
 * setnonblocking(signal_pipe) then drains with `while (read > 0)` — without
 * EAGAIN on empty, a forced-POLLIN wake hangs the session forever and the SSH
 * client never receives exit-status. */
static unsigned myos_fd_nb_mask;

void myos_fd_nonblock_set(int fd, int on) {
    if (fd < 0 || fd >= 32) {
        return;
    }
    if (on) {
        myos_fd_nb_mask |= (1u << fd);
    } else {
        myos_fd_nb_mask &= ~(1u << fd);
    }
}

int myos_fd_nonblock_get(int fd) {
    return (fd >= 0 && fd < 32 && (myos_fd_nb_mask & (1u << fd))) ? 1 : 0;
}

void myos_fd_nonblock_clear(int fd) {
    myos_fd_nonblock_set(fd, 0);
}

void myos_fd_nonblock_dup(int from, int to) {
    myos_fd_nonblock_set(to, myos_fd_nonblock_get(from));
}

/* O_CLOFORK bitmap — close these fds in the child after fork(). */
static unsigned long long myos_fd_cf_mask;

void myos_fd_clofork_set(int fd, int on) {
    if (fd < 0 || fd >= 64) {
        return;
    }
    if (on) {
        myos_fd_cf_mask |= (1ull << fd);
    } else {
        myos_fd_cf_mask &= ~(1ull << fd);
    }
}

int myos_fd_clofork_get(int fd) {
    if (fd < 0 || fd >= 64) {
        return 0;
    }
    return (myos_fd_cf_mask & (1ull << fd)) != 0;
}

void myos_fd_clofork_clear(int fd) {
    myos_fd_clofork_set(fd, 0);
}

void myos_fd_clofork_dup(int from, int to) {
    myos_fd_clofork_set(to, myos_fd_clofork_get(from));
}

void myos_fd_clofork_close_all(void) {
    int fd;
    unsigned long long m = myos_fd_cf_mask;
    myos_fd_cf_mask = 0;
    for (fd = 0; fd < 64; fd++) {
        if (m & (1ull << fd)) {
            close(fd);
        }
    }
}


int _close(int fd) {
    myos_fd_clofork_clear(fd);

    myos_socket_on_close(fd);
    long ret = myos_syscall1(MYOS_SYS_CLOSE, fd);
    if (ret == (long)MYOS_SYSERR) {
        errno = EBADF;
        return -1;
    }
    if (fd > 2) {
        myos_fd_set_tty(fd, 0);
    }
    myos_fd_path_clear(fd);
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

static long myos_kernel_oflags(int flags) {
    long k = (long)(flags & O_ACCMODE);
    if (flags & O_CREAT) {
        k |= MYOS_K_O_CREAT;
    }
    if (flags & O_TRUNC) {
        k |= MYOS_K_O_TRUNC;
    }
    if (flags & O_APPEND) {
        k |= MYOS_K_O_APPEND;
    }
    return k;
}

int _open(const char *path, int flags, ...) {
    if (path == NULL) {
        errno = ENOENT;
        return -1;
    }
    /* Writable opens are accepted for mounts that support them (tmpfs/devfs).
     * Read-only mounts are rejected by the kernel; map that to EROFS/ENOENT. */
    long ret = myos_syscall3(
        MYOS_SYS_OPEN, (long)(uintptr_t)path, (long)strlen(path),
        myos_kernel_oflags(flags));
    if (ret == (long)MYOS_SYSERR) {
        /* No controlling terminal → ENXIO (Linux open(/dev/tty) semantics). */
        errno = myos_path_is_dev_tty(path) ? ENXIO : ENOENT;
        return -1;
    }
    if (myos_path_is_tty(path)) {
        myos_fd_set_tty((int)ret, 1);
    }
    myos_fd_path_set((int)ret, path);
    myos_fd_clofork_set((int)ret, (flags & O_CLOFORK) != 0);
    return (int)ret;
}

int _read(int fd, void *buf, size_t cnt) {

    /* Pipes: POLLFD returns readiness (SYSERR = not a pipe). Honour O_NONBLOCK
     * before the blocking SYS_READ so dropbear's signal-pipe drain cannot hang. */
    if (myos_fd_nonblock_get(fd)) {
        long bits = myos_syscall1(MYOS_SYS_POLLFD, fd);
        if (bits != (long)MYOS_SYSERR && (bits & 1) == 0 && (bits & 4) == 0) {
            errno = EAGAIN;
            return -1;
        }
    }

    for (;;) {
        long ret = myos_syscall3(MYOS_SYS_READ, fd, (long)(uintptr_t)buf, (long)cnt);
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
        }
        return (int)ret;
    }
}

extern volatile long myos_sigchld_wfd;
extern volatile int myos_sigchld_insig;

int _write(int fd, const void *buf, size_t cnt) {
    long ret;

    if (myos_sigchld_insig && fd >= 0 && myos_sigchld_wfd < 0) {
        myos_sigchld_wfd = fd; /* first write in the handler = self-pipe write end */
    }
    ret = myos_syscall3(MYOS_SYS_WRITE, fd, (long)(uintptr_t)buf, (long)cnt);
    if (ret == (long)MYOS_EIO) {
        errno = EIO; /* pty peer gone */
        return -1;
    }
    if (ret == (long)MYOS_SYSERR) {
        myos_set_errno_io();
        return -1;
    }
    return (int)ret;
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

static int myos_fill_stat(struct stat *st, const struct myos_stat_buf *src)
{
    memset(st, 0, sizeof(*st));
    st->st_mode = src->st_mode;
    st->st_size = (off_t)src->st_size;
    st->st_ino = src->st_ino;
    st->st_nlink = src->st_nlink;
    st->st_dev = (dev_t)src->st_dev;
    st->st_uid = 0;
    st->st_gid = 0;
    st->st_blksize = 4096;
    st->st_blocks = (src->st_size + 511) / 512;
    return 0;
}

static int myos_stat_path(const char *path, struct stat *st)
{
    struct myos_stat_buf buf;

    if (st == NULL) {
        errno = EINVAL;
        return -1;
    }
    /* Always ask the kernel so st_dev is mount-specific (find loop checks). */
    long ret = myos_syscall3(
        MYOS_SYS_STAT,
        (long)(uintptr_t)path,
        (long)strlen(path),
        (long)(uintptr_t)&buf);
    if (ret == (long)MYOS_SYSERR) {
        errno = ENOENT;
        return -1;
    }
    return myos_fill_stat(st, &buf);
}

int _fstat(int fd, struct stat *st) {
    const char *path;

    if (st == NULL) {
        errno = EINVAL;
        return -1;
    }
    if (myos_fd_is_tty(fd)) {
        memset(st, 0, sizeof(*st));
        st->st_mode = S_IFCHR | 0666;
        st->st_rdev = (dev_t)fd;
        st->st_nlink = 1;
        return 0;
    }
    /* Prefer path-based SYS_STAT so st_size/mode match the open file.
     * A stub size of 0 made git's config mmap path treat MAP_FAILED+len0 as
     * NULL and then page-fault while rewriting .git/config (cr2≈0x23). */
    path = myos_fd_path_get(fd);
    if (path != NULL) {
        return myos_stat_path(path, st);
    }
    memset(st, 0, sizeof(*st));
    st->st_mode = S_IFREG | 0444;
    st->st_nlink = 1;
    return 0;
}

int _lstat(const char *path, struct stat *st) {
    return myos_stat_path(path, st);
}

int lstat(const char *path, struct stat *st) {
    return _lstat(path, st);
}

int _stat(const char *path, struct stat *st) {
    return myos_stat_path(path, st);
}

int _getpid(void) {
    long ret = myos_syscall0(MYOS_SYS_GETPID);
    if (ret == (long)MYOS_SYSERR) {
        return 1;
    }
    return (int)ret;
}
