/* myos libgloss: POSIX calls composed from existing primitives (no new
 * syscalls). Everything here is single-threaded-safe only, like the rest of
 * libgloss. */

#include <errno.h>
#include <fcntl.h>
#include <limits.h>
#include <signal.h>
#include <stdarg.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>
#include <sys/lock.h>
#include <sys/random.h>
#include <unistd.h>

#include "myos_syscalls.h"

void *memalign(size_t align, size_t size); /* newlib malloc.h */

int dup(int fd) {
    return fcntl(fd, F_DUPFD, 0);
}

int dup3(int oldfd, int newfd, int flags) {
    if (oldfd == newfd || (flags & ~O_CLOEXEC) != 0) {
        errno = EINVAL;
        return -1;
    }
    if (dup2(oldfd, newfd) < 0) {
        return -1;
    }
    if (flags & O_CLOEXEC) {
        fcntl(newfd, F_SETFD, FD_CLOEXEC);
    }
    return newfd;
}

int pipe2(int fds[2], int flags) {
    if ((flags & ~(O_CLOEXEC | O_NONBLOCK)) != 0) {
        errno = EINVAL;
        return -1;
    }
    if (myos_pipe_flags(fds, flags & O_CLOEXEC) < 0) {
        return -1;
    }
    if (flags & O_NONBLOCK) {
        fcntl(fds[0], F_SETFL, O_NONBLOCK);
        fcntl(fds[1], F_SETFL, O_NONBLOCK);
    }
    return 0;
}

/* tmpfs/ext2 writes are synchronous from userspace's point of view. */
int fsync(int fd) {
    struct stat st;
    return fstat(fd, &st);
}

int fdatasync(int fd) {
    return fsync(fd);
}

/* A stream's own lock, the recursive one newlib's stdio takes around each
 * call (pthread.c makes it a mutex): a thread holds it across several. */
void flockfile(FILE *f) {
    _flockfile(f);
}

int ftrylockfile(FILE *f) {
    if (f->_flags & __SSTR) {
        return 0;
    }
    return __lock_try_acquire_recursive(f->_lock) ? 0 : -1;
}

void funlockfile(FILE *f) {
    _funlockfile(f);
}

int killpg(int pgrp, int sig) {
    if (pgrp <= 1) {
        errno = EINVAL;
        return -1;
    }
    return kill(-pgrp, sig);
}

int posix_memalign(void **out, size_t align, size_t size) {
    void *p;
    if (align < sizeof(void *) || (align & (align - 1)) != 0) {
        return EINVAL;
    }
    p = memalign(align, size ? size : 1);
    if (p == NULL) {
        return ENOMEM;
    }
    *out = p;
    return 0;
}

int truncate(const char *path, off_t length) {
    int fd = open(path, O_WRONLY);
    int ret;
    if (fd < 0) {
        return -1;
    }
    ret = ftruncate(fd, length);
    close(fd);
    return ret;
}

extern char **environ;

int execv(const char *path, char *const argv[]) {
    return execve(path, argv, environ);
}

/* execl/execle: collect the variadic list into an argv on the stack. */
#define MYOS_EXECL_MAX 64

int execl(const char *path, const char *arg0, ...) {
    char *argv[MYOS_EXECL_MAX + 1];
    va_list ap;
    int n = 0;
    argv[n++] = (char *)arg0;
    va_start(ap, arg0);
    while (argv[n - 1] != NULL) {
        if (n > MYOS_EXECL_MAX) {
            va_end(ap);
            errno = E2BIG;
            return -1;
        }
        argv[n++] = va_arg(ap, char *);
    }
    va_end(ap);
    return execve(path, argv, environ);
}

int execle(const char *path, const char *arg0, ...) {
    char *argv[MYOS_EXECL_MAX + 1];
    char **envp;
    va_list ap;
    int n = 0;
    argv[n++] = (char *)arg0;
    va_start(ap, arg0);
    while (argv[n - 1] != NULL) {
        if (n > MYOS_EXECL_MAX) {
            va_end(ap);
            errno = E2BIG;
            return -1;
        }
        argv[n++] = va_arg(ap, char *);
    }
    envp = va_arg(ap, char **);
    va_end(ap);
    return execve(path, argv, envp);
}

/* vfork: a fork. The child may do anything a forked child may, which is
 * more than vfork promises, not less. */
pid_t vfork(void) {
    return fork();
}

/* daemon(3): fork, let the parent exit, a session of its own; `nochdir`
 * keeps the working directory, `noclose` keeps the standard descriptors
 * (else they go to /dev/null). */
int daemon(int nochdir, int noclose) {
    pid_t pid = fork();
    if (pid < 0) {
        return -1;
    }
    if (pid > 0) {
        _exit(0);
    }
    if (setsid() < 0) {
        return -1;
    }
    if (!nochdir && chdir("/") < 0) {
        return -1;
    }
    if (!noclose) {
        int fd = open("/dev/null", O_RDWR);
        if (fd < 0) {
            return -1;
        }
        if (dup2(fd, 0) < 0 || dup2(fd, 1) < 0 || dup2(fd, 2) < 0) {
            close(fd);
            return -1;
        }
        if (fd > 2) {
            close(fd);
        }
    }
    return 0;
}

/* getrandom(2) from /dev/urandom (the kernel's generator, docs/testing.md
 * `urandom`): never blocks, so GRND_NONBLOCK and GRND_RANDOM change nothing. */
ssize_t getrandom(void *buf, size_t buflen, unsigned int flags) {
    if ((flags & ~(GRND_NONBLOCK | GRND_RANDOM | GRND_INSECURE)) != 0) {
        errno = EINVAL;
        return -1;
    }
    int fd = open("/dev/urandom", O_RDONLY);
    if (fd < 0) {
        return -1;
    }
    size_t got = 0;
    while (got < buflen) {
        ssize_t n = read(fd, (char *)buf + got, buflen - got);
        if (n < 0) {
            if (errno == EINTR && got == 0) {
                continue;
            }
            int e = errno;
            close(fd);
            if (got > 0) {
                return (ssize_t)got;
            }
            errno = e;
            return -1;
        }
        if (n == 0) {
            break;
        }
        got += (size_t)n;
    }
    close(fd);
    return (ssize_t)got;
}
