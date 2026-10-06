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

static int myos_rofs(void) {
    errno = EROFS;
    return -1;
}

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

/* Phase-1 git index writes call ftruncate after writing the full blob.
 * No SYS_FTRUNCATE yet; treat as success so commit/add work on tmpfs when
 * the fd already holds the intended bytes (hashfile write path). */
int ftruncate(int fd, off_t length) {
    (void)fd;
    (void)length;
    return 0;
}

int pipe(int fildes[2]) {
    unsigned long fds[2];
    long ret;

    if (fildes == NULL) {
        errno = EFAULT;
        return -1;
    }
    /* Kernel writes usize[2], not int[2]. */
    ret = myos_syscall1(MYOS_SYS_PIPE, (long)(uintptr_t)fds);
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
    (void)fd;
    (void)operation;
    return myos_nosys();
}

int linkat(int olddirfd, const char *oldpath, int newdirfd, const char *newpath, int flags) {
    (void)olddirfd;
    (void)oldpath;
    (void)newdirfd;
    (void)newpath;
    (void)flags;
    return myos_rofs();
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

/* No interval timers: callers fall back (the X server's scheduler runs
 * without its SIGALRM time slices). */
int setitimer(int which, const struct itimerval *value, struct itimerval *old) {
    (void)which;
    (void)value;
    (void)old;
    return myos_nosys();
}

int getitimer(int which, struct itimerval *value) {
    (void)which;
    (void)value;
    return myos_nosys();
}
