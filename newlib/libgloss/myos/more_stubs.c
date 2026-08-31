#include <errno.h>
#include <fcntl.h>
#include <fnmatch.h>
#include <limits.h>
#include <signal.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/resource.h>
#include <sys/stat.h>
#include <sys/utsname.h>
#include <termios.h>
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

static void myos_fill_termios(struct termios *t) {
    memset(t, 0, sizeof(*t));
#ifdef ICANON
    t->c_lflag = ICANON | ECHO | ISIG;
#endif
#ifdef IGNCR
    t->c_iflag = IGNCR;
#endif
#ifdef VINTR
    t->c_cc[VINTR] = 3;
    t->c_cc[VQUIT] = 28;
    t->c_cc[VERASE] = 8;
    t->c_cc[VKILL] = 21;
    t->c_cc[VEOF] = 4;
    t->c_cc[VMIN] = 1;
    t->c_cc[VTIME] = 0;
#endif
}

/* Kernel stdin is already line-buffered; report a cooked tty and ignore sets. */
int tcgetattr(int fd, struct termios *t) {
    if (t == NULL) {
        errno = EINVAL;
        return -1;
    }
    if (fd < 0) {
        errno = EBADF;
        return -1;
    }
    myos_fill_termios(t);
    return 0;
}

int tcsetattr(int fd, int optional_actions, const struct termios *t) {
    (void)optional_actions;
    (void)t;
    if (fd < 0) {
        errno = EBADF;
        return -1;
    }
    return 0;
}

pid_t tcgetpgrp(int fd) {
    (void)fd;
    return 0;
}

int tcsetpgrp(int fd, pid_t pgrp) {
    (void)fd;
    (void)pgrp;
    return 0;
}

int setpgid(pid_t pid, pid_t pgid) {
    (void)pid;
    (void)pgid;
    return 0;
}

pid_t getppid(void) {
    return 0;
}

gid_t getegid(void) {
    return 0;
}

int setgroups(int size, const gid_t *list) {
    (void)size;
    (void)list;
    return 0;
}

pid_t getpgid(pid_t pid) {
    (void)pid;
    return 0;
}

pid_t getsid(pid_t pid) {
    (void)pid;
    return 0;
}

unsigned int alarm(unsigned int seconds) {
    (void)seconds;
    return 0;
}

int getrusage(int who, struct rusage *usage) {
    (void)who;
    if (usage == NULL) {
        errno = EFAULT;
        return -1;
    }
    memset(usage, 0, sizeof(*usage));
    return 0;
}

#ifndef RLIM_INFINITY
#define RLIM_INFINITY ((rlim_t)-1)
#endif

int getrlimit(int resource, struct rlimit *rlp) {
    (void)resource;
    if (rlp == NULL) {
        errno = EFAULT;
        return -1;
    }
    rlp->rlim_cur = RLIM_INFINITY;
    rlp->rlim_max = RLIM_INFINITY;
    return 0;
}

int setrlimit(int resource, const struct rlimit *rlp) {
    (void)resource;
    (void)rlp;
    return 0;
}

int setuid(uid_t uid) {
    (void)uid;
    return 0;
}

int seteuid(uid_t uid) {
    (void)uid;
    return 0;
}

int setgid(gid_t gid) {
    (void)gid;
    return 0;
}

int setegid(gid_t gid) {
    (void)gid;
    return 0;
}

size_t confstr(int name, char *buf, size_t len) {
    static const char path[] = "/:/s:/c";
    size_t n = sizeof(path); /* includes NUL */

    (void)name;
    if (buf != NULL && len > 0) {
        size_t copy = n < len ? n : len;
        memcpy(buf, path, copy);
        buf[len - 1] = '\0';
    }
    return n;
}

int sigsuspend(const sigset_t *mask) {
    (void)mask;
    errno = ENOSYS;
    return -1;
}

int gethostname(char *name, size_t len) {
    static const char host[] = "myos";
    size_t n = sizeof(host);

    if (name == NULL) {
        errno = EFAULT;
        return -1;
    }
    if (len == 0) {
        errno = EINVAL;
        return -1;
    }
    if (n > len) {
        memcpy(name, host, len);
        name[len - 1] = '\0';
        errno = ENAMETOOLONG;
        return -1;
    }
    memcpy(name, host, n);
    return 0;
}

int killpg(pid_t pgrp, int sig) {
    (void)pgrp;
    (void)sig;
    errno = ENOSYS;
    return -1;
}

int nice(int inc) {
    (void)inc;
    return 0;
}
