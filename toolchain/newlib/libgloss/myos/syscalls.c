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

int _close(int fd) {
    long ret = myos_syscall1(MYOS_SYS_CLOSE, fd);
    if (ret == (long)MYOS_SYSERR) {
        errno = EBADF;
        return -1;
    }
    if (fd > 2) {
        myos_fd_set_tty(fd, 0);
    }
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
    return (int)ret;
}

int _read(int fd, void *buf, size_t cnt) {
    long ret = myos_syscall3(MYOS_SYS_READ, fd, (long)(uintptr_t)buf, (long)cnt);
    if (ret == (long)MYOS_SYSERR) {
        errno = EBADF;
        return -1;
    }
    return (int)ret;
}

int _write(int fd, const void *buf, size_t cnt) {
    long ret = myos_syscall3(MYOS_SYS_WRITE, fd, (long)(uintptr_t)buf, (long)cnt);
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
    if (st == NULL) {
        errno = EINVAL;
        return -1;
    }
    memset(st, 0, sizeof(*st));
    if (myos_fd_is_tty(fd)) {
        st->st_mode = S_IFCHR | 0666;
        st->st_rdev = (dev_t)fd;
        st->st_nlink = 1;
        return 0;
    }
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
