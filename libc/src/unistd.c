#include <myos/syscall.h>
#include <stddef.h>
#include <stdint.h>
#include <string.h>
#include <unistd.h>

ssize_t write(int fd, const void *buf, size_t len) {
    long ret = myos_syscall3(MYOS_SYS_WRITE, fd, (long)(uintptr_t)buf, (long)len);
    if (ret == (long)MYOS_SYSERR) {
        return -1;
    }
    return (ssize_t)ret;
}

ssize_t read(int fd, void *buf, size_t len) {
    long ret = myos_syscall3(MYOS_SYS_READ, fd, (long)(uintptr_t)buf, (long)len);
    if (ret == (long)MYOS_SYSERR) {
        return -1;
    }
    return (ssize_t)ret;
}

int open(const char *path) {
    long ret = myos_syscall3(MYOS_SYS_OPEN, (long)(uintptr_t)path, (long)strlen(path), 0);
    if (ret == (long)MYOS_SYSERR) {
        return -1;
    }
    return (int)ret;
}

int close(int fd) {
    long ret = myos_syscall1(MYOS_SYS_CLOSE, fd);
    if (ret == (long)MYOS_SYSERR) {
        return -1;
    }
    return 0;
}

void _exit(int code) {
    myos_syscall1(MYOS_SYS_EXIT, code);
    for (;;) {
    }
}

int fork(void) {
    long ret = myos_syscall0(MYOS_SYS_FORK);
    if (ret == (long)MYOS_SYSERR) {
        return -1;
    }
    return (int)ret;
}

int wait(int *status) {
    long ret = myos_syscall1(MYOS_SYS_WAIT, (long)(uintptr_t)status);
    if (ret == (long)MYOS_SYSERR) {
        return -1;
    }
    return (int)ret;
}

int pipe(int fds[2]) {
    long ret = myos_syscall1(MYOS_SYS_PIPE, (long)(uintptr_t)fds);
    if (ret == (long)MYOS_SYSERR) {
        return -1;
    }
    return 0;
}

int dup2(int oldfd, int newfd) {
    long ret = myos_syscall3(MYOS_SYS_DUP2, oldfd, newfd, 0);
    if (ret == (long)MYOS_SYSERR) {
        return -1;
    }
    return (int)ret;
}

void *sbrk(intptr_t inc) {
    static void *cur;
    if (cur == 0) {
        cur = (void *)(uintptr_t)myos_syscall1(MYOS_SYS_BRK, 0);
    }
    void *old = cur;
    void *next = (char *)cur + inc;
    void *got = (void *)(uintptr_t)myos_syscall1(MYOS_SYS_BRK, (long)(uintptr_t)next);
    if (got != next) {
        return (void *)-1;
    }
    cur = next;
    return old;
}
