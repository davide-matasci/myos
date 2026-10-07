#include <errno.h>
#include <stdarg.h>
#include <fcntl.h>
#include <fnmatch.h>
#include <limits.h>
#include <signal.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>
#include <sys/time.h>
#include <sys/utsname.h>
#include <unistd.h>

#include "myos_syscalls.h"

static int myos_nosys(void) {
    errno = ENOSYS;
    return -1;
}

int fchmodat(int dirfd, const char *path, mode_t mode, int flags) {
    (void)dirfd;
    (void)path;
    (void)mode;
    (void)flags;
    /* Match chmod/fchmod: no mode bits yet. */
    return 0;
}

int fchown(int fd, uid_t owner, gid_t group) {
    (void)fd;
    (void)owner;
    (void)group;
    /* Always root; getty fchown on the console tty. */
    return 0;
}

int fchmod(int fd, mode_t mode) {
    (void)fd;
    (void)mode;
    return 0;
}

int vhangup(void) {
    return 0;
}

int initgroups(const char *user, gid_t group) {
    (void)user;
    (void)group;
    return 0;
}

#define MYOS_HOSTNAME_FILE "/proc/sys/kernel/hostname"

/* The kernel keeps the host name in /proc/sys/kernel/hostname ("myos" at
 * boot); n bytes of it without the newline go to buf. */
static int myos_hostname_read(char *buf, size_t size, size_t *n) {
    int fd = open(MYOS_HOSTNAME_FILE, O_RDONLY);
    ssize_t r;
    if (fd < 0) {
        return -1;
    }
    r = read(fd, buf, size);
    close(fd);
    if (r < 0) {
        return -1;
    }
    while (r > 0 && buf[r - 1] == '\n') {
        r--;
    }
    *n = (size_t)r;
    return 0;
}

int gethostname(char *name, size_t len) {
    char hn[72];
    size_t n;
    if (name == NULL || len == 0) {
        errno = EINVAL;
        return -1;
    }
    if (myos_hostname_read(hn, sizeof hn, &n) < 0) {
        memcpy(hn, "myos", 4);
        n = 4;
    }
    if (n + 1 > len) {
        errno = ENAMETOOLONG;
        return -1;
    }
    memcpy(name, hn, n);
    name[n] = '\0';
    return 0;
}

int sethostname(const char *name, size_t len) {
    int fd;
    ssize_t w;
    if (name == NULL && len != 0) {
        errno = EFAULT;
        return -1;
    }
    if (len > 64 || memchr(name, '\n', len) != NULL) {
        errno = EINVAL;
        return -1;
    }
    if ((fd = open(MYOS_HOSTNAME_FILE, O_WRONLY | O_TRUNC)) < 0) {
        return -1;
    }
    /* an empty name is written as its newline: a write of 0 bytes is none */
    w = len != 0 ? write(fd, name, len) : write(fd, "\n", 1);
    close(fd);
    if (w < 0) {
        errno = EINVAL;
        return -1;
    }
    return 0;
}

int ftruncate(int fd, off_t length) {
    if (length < 0) {
        errno = EINVAL;
        return -1;
    }
    if (myos_syscall3(MYOS_SYS_FTRUNCATE, fd, (long)length, 0) == (long)MYOS_SYSERR) {
        /* Not an fd, not open for writing (EBADF), or not a file that can
         * be resized (EINVAL). */
        errno = myos_syscall3(MYOS_SYS_FDFLAGS, fd, MYOS_FD_GET, 0) == (long)MYOS_SYSERR ? EBADF : EINVAL;
        return -1;
    }
    return 0;
}

/* pipe2's O_CLOEXEC: both ends close at exec. */
int myos_pipe_flags(int fildes[2], int cloexec) {
    unsigned long fds[2];
    long ret;

    if (fildes == NULL) {
        errno = EFAULT;
        return -1;
    }
    /* Kernel writes usize[2], not int[2]. */
    ret = myos_syscall2(MYOS_SYS_PIPE, (long)(uintptr_t)fds, cloexec ? MYOS_FD_CLOEXEC : 0);
    if (ret == (long)MYOS_SYSERR) {
        errno = EMFILE;
        return -1;
    }
    fildes[0] = (int)fds[0];
    fildes[1] = (int)fds[1];
    myos_fd_nonblock_clear(fildes[0]);
    myos_fd_nonblock_clear(fildes[1]);
    return 0;
}

int pipe(int fildes[2]) {
    return myos_pipe_flags(fildes, 0);
}

int _pipe(int fildes[2]) {
    return pipe(fildes);
}

FILE *popen(const char *command, const char *type) {
    (void)command;
    (void)type;
    return NULL;
}

int pclose(FILE *stream) {
    (void)stream;
    return myos_nosys();
}

int execlp(const char *file, const char *arg, ...) {
    char *argv[16];
    va_list ap;
    int i = 0;

    if (file == NULL) {
        errno = ENOENT;
        return -1;
    }
    argv[i++] = (char *)arg;
    va_start(ap, arg);
    while (i < 15) {
        char *a = va_arg(ap, char *);
        argv[i] = a;
        if (a == NULL) {
            break;
        }
        i++;
    }
    va_end(ap);
    argv[15] = NULL;
    return execvp(file, argv);
}

char *realpath(const char *path, char *resolved) {
    if (path == NULL) {
        errno = EINVAL;
        return NULL;
    }
    if (resolved != NULL) {
        if (strlen(path) >= PATH_MAX) {
            errno = ENAMETOOLONG;
            return NULL;
        }
        strcpy(resolved, path);
        return resolved;
    }
    size_t n = strlen(path) + 1;
    char *out = malloc(n);
    if (out == NULL) {
        return NULL;
    }
    memcpy(out, path, n);
    return out;
}

long pathconf(const char *path, int name) {
    (void)path;
    (void)name;
    errno = ENOSYS;
    return -1;
}

int getpriority(int which, id_t who) {
    (void)which;
    (void)who;
    return 0;
}

pid_t getpgrp(void) {
    return getpgid(0);
}

int flock(int fd, int operation) {
    long ret = myos_syscall2(MYOS_SYS_FLOCK, fd, operation);
    if (ret == (long)MYOS_EAGAIN) {
        errno = EWOULDBLOCK;
        return -1;
    }
    if (ret == (long)MYOS_EINTR) {
        errno = EINTR;
        return -1;
    }
    if (ret == (long)MYOS_SYSERR) {
        int op = operation & ~LOCK_NB;
        errno = op == LOCK_SH || op == LOCK_EX || op == LOCK_UN ? EBADF : EINVAL;
        return -1;
    }
    return 0;
}

/* lockf: an exclusive record lock from the position on (`len` 0: to the
 * end, negative: the bytes before it), through fcntl's. */
int lockf(int fd, int cmd, off_t len) {
    struct flock fl = {.l_whence = SEEK_CUR, .l_start = 0, .l_len = len};
    switch (cmd) {
    case F_ULOCK:
        fl.l_type = F_UNLCK;
        return fcntl(fd, F_SETLK, &fl);
    case F_LOCK:
        fl.l_type = F_WRLCK;
        return fcntl(fd, F_SETLKW, &fl);
    case F_TLOCK:
        fl.l_type = F_WRLCK;
        return fcntl(fd, F_SETLK, &fl);
    case F_TEST:
        fl.l_type = F_WRLCK;
        if (fcntl(fd, F_GETLK, &fl) < 0) {
            return -1;
        }
        if (fl.l_type == F_UNLCK) {
            return 0;
        }
        errno = EACCES;
        return -1;
    default:
        errno = EINVAL;
        return -1;
    }
}

/* No hard links (see _link). */
int linkat(int olddirfd, const char *oldpath, int newdirfd, const char *newpath, int flags) {
    (void)olddirfd;
    (void)oldpath;
    (void)newdirfd;
    (void)newpath;
    (void)flags;
    errno = EPERM;
    return -1;
}

/* Real SYS_SIGPROCMASK: kernel keeps a per-task blocked mask and returns the
 * old mask. The kernel reads and writes the mask as 32 bits; sigset_t is one
 * unsigned long (sys/_sigset.h), so go through 32-bit temporaries or the
 * upper half of *oset keeps whatever was there. */
int sigprocmask(int how, const sigset_t *restrict set, sigset_t *restrict oset) {
    unsigned int kset = 0;
    unsigned int kold = 0;
    long ret;

    if (set != NULL) {
        kset = (unsigned int)*set;
    }
    ret = myos_syscall3(MYOS_SYS_SIGPROCMASK, (long)how,
                        set ? (long)(uintptr_t)&kset : 0,
                        oset ? (long)(uintptr_t)&kold : 0);
    if (ret == (long)MYOS_SYSERR) {
        errno = EINVAL;
        return -1;
    }
    if (oset != NULL) {
        *oset = (sigset_t)kold;
    }
    return 0;
}

int uname(struct utsname *buf) {
    if (buf == NULL) {
        errno = EFAULT;
        return -1;
    }
    strncpy(buf->sysname, "myos", sizeof(buf->sysname));
    if (gethostname(buf->nodename, sizeof(buf->nodename)) < 0) {
        strncpy(buf->nodename, "myos", sizeof(buf->nodename));
    }
    strncpy(buf->release, "0.1", sizeof(buf->release));
    strncpy(buf->version, "myos", sizeof(buf->version));
    strncpy(buf->machine, "myos", sizeof(buf->machine));
    return 0;
}

pid_t getppid(void) {
    return (pid_t)myos_syscall0(MYOS_SYS_GETPPID);
}

/* 4 KiB pages on all three arches. */
int getpagesize(void) {
    return 4096;
}

int getdtablesize(void) {
    return MYOS_OPEN_MAX;
}

/* The interval timers: ITIMER_REAL (SIGALRM) is the kernel's (itimer);
 * ITIMER_VIRTUAL and ITIMER_PROF would need CPU time accounting. */
static unsigned long long tv_us(const struct timeval *tv) {
    return (unsigned long long)tv->tv_sec * 1000000ULL + (unsigned long long)tv->tv_usec;
}

static void us_tv(unsigned long long us, struct timeval *tv) {
    tv->tv_sec = (time_t)(us / 1000000ULL);
    tv->tv_usec = (suseconds_t)(us % 1000000ULL);
}

static int itimer(int which, const struct itimerval *value, struct itimerval *old) {
    unsigned long long set[2], was[2];
    if (which == ITIMER_VIRTUAL || which == ITIMER_PROF) {
        return myos_nosys();
    }
    if (which != ITIMER_REAL || (value && (value->it_value.tv_usec < 0 || value->it_value.tv_usec >= 1000000 ||
                                            value->it_interval.tv_usec < 0 || value->it_interval.tv_usec >= 1000000))) {
        errno = EINVAL;
        return -1;
    }
    if (value) {
        set[0] = tv_us(&value->it_value);
        set[1] = tv_us(&value->it_interval);
    }
    if (myos_syscall3(MYOS_SYS_ITIMER, 0, value ? (long)set : 0, old ? (long)was : 0) == (long)MYOS_SYSERR) {
        errno = EFAULT;
        return -1;
    }
    if (old) {
        us_tv(was[0], &old->it_value);
        us_tv(was[1], &old->it_interval);
    }
    return 0;
}

int setitimer(int which, const struct itimerval *value, struct itimerval *old) {
    if (!value) {
        errno = EFAULT;
        return -1;
    }
    return itimer(which, value, old);
}

int getitimer(int which, struct itimerval *value) {
    if (!value) {
        errno = EFAULT;
        return -1;
    }
    return itimer(which, NULL, value);
}

/* SIGALRM in `seconds` (0 cancels); the seconds that were left of the
 * previous alarm, rounded up. */
unsigned alarm(unsigned seconds) {
    struct itimerval it = {{0, 0}, {(time_t)seconds, 0}}, old;
    if (itimer(ITIMER_REAL, &it, &old) != 0) {
        return 0;
    }
    return (unsigned)old.it_value.tv_sec + (old.it_value.tv_usec != 0);
}
