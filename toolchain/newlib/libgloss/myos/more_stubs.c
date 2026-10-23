/* myos libgloss: the POSIX calls newlib's libc lacks that are only libc
 * (over the calls of toolchain/libmyos): vhangup, initgroups, popen, execlp,
 * realpath, pathconf, lockf, alarm, the page size and fd table size. */
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
#include <unistd.h>

#include "myos_syscalls.h"

static int myos_nosys(void) {
    errno = ENOSYS;
    return -1;
}

int vhangup(void) {
    return 0;
}

int initgroups(const char *user, gid_t group) {
    (void)user;
    (void)group;
    return 0;
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

/* 4 KiB pages on all three arches. */
int getpagesize(void) {
    return 4096;
}

int getdtablesize(void) {
    return MYOS_OPEN_MAX;
}

/* SIGALRM in `seconds` (0 cancels); the seconds that were left of the
 * previous alarm, rounded up. */
unsigned alarm(unsigned seconds) {
    struct itimerval it = {{0, 0}, {(time_t)seconds, 0}}, old;
    if (setitimer(ITIMER_REAL, &it, &old) != 0) {
        return 0;
    }
    return (unsigned)old.it_value.tv_sec + (old.it_value.tv_usec != 0);
}
