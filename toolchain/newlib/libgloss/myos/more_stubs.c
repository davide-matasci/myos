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
    return myos_rofs();
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

int gethostname(char *name, size_t len) {
    const char *hn = "myos";
    size_t n;
    if (name == NULL || len == 0) {
        errno = EINVAL;
        return -1;
    }
    n = strlen(hn);
    if (n + 1 > len) {
        errno = ENAMETOOLONG;
        return -1;
    }
    memcpy(name, hn, n + 1);
    return 0;
}

int ftruncate(int fd, off_t length) {
    (void)fd;
    (void)length;
    return myos_nosys();
}

int mkfifo(const char *path, mode_t mode) {
    (void)path;
    (void)mode;
    return myos_rofs();
}

int rmdir(const char *path) {
    long ret;
    if (path == NULL) {
        errno = ENOENT;
        return -1;
    }
    ret = myos_syscall3(
        MYOS_SYS_RMDIR, (long)(uintptr_t)path, (long)strlen(path), 0);
    if (ret == (long)MYOS_SYSERR) {
        errno = EROFS;
        return -1;
    }
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

char *getlogin(void) {
    return NULL;
}

uid_t geteuid(void) {
    return 0;
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

int sigprocmask(int how, const sigset_t *restrict set, sigset_t *restrict oset) {
    (void)how;
    (void)set;
    (void)oset;
    return 0;
}

int chroot(const char *path) {
    (void)path;
    return myos_nosys();
}

int uname(struct utsname *buf) {
    if (buf == NULL) {
        errno = EFAULT;
        return -1;
    }
    strncpy(buf->sysname, "myos", sizeof(buf->sysname));
    strncpy(buf->nodename, "myos", sizeof(buf->nodename));
    strncpy(buf->release, "0.1", sizeof(buf->release));
    strncpy(buf->version, "myos", sizeof(buf->version));
    strncpy(buf->machine, "myos", sizeof(buf->machine));
    return 0;
}
