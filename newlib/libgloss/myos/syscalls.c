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

static int myos_err(long ret) {
    if (ret == (long)MYOS_SYSERR) {
        return -1;
    }
    return (int)ret;
}

static void myos_set_errno_io(void) {
    errno = EIO;
}

int _close(int fd) {
    long ret = myos_syscall1(MYOS_SYS_CLOSE, fd);
    if (ret == (long)MYOS_SYSERR) {
        errno = EBADF;
        return -1;
    }
    return 0;
}

void _exit(int status) {
    myos_syscall1(MYOS_SYS_EXIT, status);
    for (;;) {
    }
}

int _open(const char *path, int flags, ...) {
    (void)flags;
    if (path == NULL) {
        errno = ENOENT;
        return -1;
    }
    if (flags & (O_WRONLY | O_RDWR | O_CREAT | O_TRUNC | O_APPEND)) {
        errno = EROFS;
        return -1;
    }
    long ret = myos_syscall3(MYOS_SYS_OPEN, (long)(uintptr_t)path, (long)strlen(path), 0);
    if (ret == (long)MYOS_SYSERR) {
        errno = ENOENT;
        return -1;
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
    if (fd >= 0 && fd <= 2) {
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

int _fstat(int fd, struct stat *st) {
    if (st == NULL) {
        errno = EINVAL;
        return -1;
    }
    memset(st, 0, sizeof(*st));
    if (fd >= 0 && fd <= 2) {
        st->st_mode = S_IFCHR | 0666;
        return 0;
    }
    st->st_mode = S_IFREG | 0444;
    return 0;
}

int _stat(const char *path, struct stat *st) {
    int fd = _open(path, O_RDONLY, 0);
    if (fd < 0) {
        return -1;
    }
    int rc = _fstat(fd, st);
    _close(fd);
    return rc;
}

int _getpid(void) {
    return 1;
}
