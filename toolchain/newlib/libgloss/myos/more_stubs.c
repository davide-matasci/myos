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
#include <sys/wait.h>
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

/* popen and pclose: the command runs under /bin/sh with one end of a pipe
 * as its standard output ("r") or input ("w"); pclose waits for it. Which
 * child a stream belongs to is kept in a list (one thread's at a time: the
 * list has no lock). */
struct popen_child {
    struct popen_child *next;
    FILE *stream;
    pid_t pid;
};
static struct popen_child *popen_children;

FILE *popen(const char *command, const char *type) {
    struct popen_child *c;
    int fds[2];
    int reading;
    int mine;
    int theirs;

    if (command == NULL || type == NULL || (type[0] != 'r' && type[0] != 'w')) {
        errno = EINVAL;
        return NULL;
    }
    reading = type[0] == 'r';
    c = malloc(sizeof(*c));
    if (c == NULL) {
        return NULL;
    }
    if (pipe(fds) < 0) {
        free(c);
        return NULL;
    }
    mine = reading ? fds[0] : fds[1];
    theirs = reading ? fds[1] : fds[0];
    c->pid = fork();
    if (c->pid < 0) {
        close(fds[0]);
        close(fds[1]);
        free(c);
        return NULL;
    }
    if (c->pid == 0) {
        int std = reading ? STDOUT_FILENO : STDIN_FILENO;
        struct popen_child *o;

        /* The command holds no end of an earlier popen's pipe (POSIX): its
         * reader would never see the end of the data otherwise. */
        for (o = popen_children; o != NULL; o = o->next) {
            close(fileno(o->stream));
        }
        close(mine);
        if (theirs != std) {
            dup2(theirs, std);
            close(theirs);
        }
        execl("/bin/sh", "sh", "-c", command, (char *)NULL);
        _exit(127);
    }
    close(theirs);
    c->stream = fdopen(mine, reading ? "r" : "w");
    if (c->stream == NULL) {
        int e = errno;
        int st;

        close(mine);
        waitpid(c->pid, &st, 0);
        free(c);
        errno = e;
        return NULL;
    }
    c->next = popen_children;
    popen_children = c;
    return c->stream;
}

int pclose(FILE *stream) {
    struct popen_child **link;
    struct popen_child *c;
    pid_t pid;
    int st;

    for (link = &popen_children; *link != NULL; link = &(*link)->next) {
        if ((*link)->stream == stream) {
            break;
        }
    }
    c = *link;
    if (c == NULL) {
        errno = ECHILD;
        return -1;
    }
    *link = c->next;
    pid = c->pid;
    free(c);
    fclose(stream);
    while (waitpid(pid, &st, 0) < 0) {
        if (errno != EINTR) {
            return -1;
        }
    }
    return st;
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
