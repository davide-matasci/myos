#include <errno.h>
#include <fcntl.h>
#include <fnmatch.h>
#include <limits.h>
#include <signal.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>
#include <sys/utsname.h>
#include <unistd.h>

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
    return myos_rofs();
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
    (void)path;
    return myos_rofs();
}

int pipe(int fildes[2]) {
    (void)fildes;
    return myos_nosys();
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
    (void)file;
    (void)arg;
    return myos_nosys();
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
    return 0;
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
