/* myos libgloss: POSIX helpers sbase expects beyond read-only VFS hooks. */

#include <dirent.h>
#include <errno.h>
#include <fcntl.h>
#include <grp.h>
#include <pwd.h>
#include <stdint.h>
#include <string.h>
#include <sys/stat.h>
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

int access(const char *path, int mode) {
    struct stat st;
    (void)mode;
    if (path == NULL) {
        errno = EINVAL;
        return -1;
    }
    if (stat(path, &st) < 0) {
        return -1;
    }
    return 0;
}

int creat(const char *path, mode_t mode) {
    (void)path;
    (void)mode;
    return myos_rofs();
}

int chmod(const char *path, mode_t mode) {
    (void)path;
    (void)mode;
    return myos_rofs();
}

int mkdir(const char *path, mode_t mode) {
    (void)path;
    (void)mode;
    return myos_rofs();
}

mode_t umask(mode_t mask) {
    static mode_t cur = 022;
    mode_t old = cur;
    cur = mask;
    return old;
}

int symlink(const char *target, const char *linkpath) {
    (void)target;
    (void)linkpath;
    return myos_rofs();
}

int mknod(const char *path, mode_t mode, dev_t dev) {
    (void)path;
    (void)mode;
    (void)dev;
    return myos_rofs();
}

int chown(const char *path, uid_t owner, gid_t group) {
    (void)path;
    (void)owner;
    (void)group;
    return myos_rofs();
}

int lchown(const char *path, uid_t owner, gid_t group) {
    (void)path;
    (void)owner;
    (void)group;
    return myos_rofs();
}

int openat(int dirfd, const char *path, int flags, ...) {
    (void)dirfd;
    (void)path;
    (void)flags;
    return myos_nosys();
}

int faccessat(int dirfd, const char *path, int mode, int flags) {
    (void)dirfd;
    (void)flags;
    return access(path, mode);
}

int fstatat(int dirfd, const char *path, struct stat *st, int flags) {
    (void)dirfd;
    (void)flags;
    if (st == NULL) {
        errno = EINVAL;
        return -1;
    }
    return stat(path, st);
}

int unlinkat(int dirfd, const char *path, int flags) {
    (void)dirfd;
    (void)path;
    (void)flags;
    return myos_rofs();
}

int utimensat(int dirfd, const char *path, const struct timespec times[2], int flags) {
    (void)dirfd;
    (void)path;
    (void)times;
    (void)flags;
    return myos_rofs();
}

DIR *fdopendir(int fd) {
    (void)fd;
    errno = ENOSYS;
    return NULL;
}

int execvp(const char *file, char *const argv[]) {
    (void)file;
    (void)argv;
    return myos_nosys();
}

/*
 * Kernel SYS_WAIT is wait-any and blocking; it stores a raw exit-code byte.
 * Ignore WNOHANG/WUNTRACED/specific pid and convert to POSIX wait status.
 */
pid_t waitpid(pid_t pid, int *status, int options) {
    unsigned char code = 0;
    long ret;

    (void)pid;
    (void)options;
    ret = myos_syscall1(MYOS_SYS_WAIT, status ? (long)(uintptr_t)&code : 0);
    if (ret == (long)MYOS_SYSERR) {
        errno = ECHILD;
        return -1;
    }
    if (status != NULL) {
        *status = ((int)code) << 8;
    }
    return (pid_t)ret;
}

long sysconf(int name) {
    (void)name;
    errno = ENOSYS;
    return -1;
}

unsigned sleep(unsigned seconds) {
    (void)seconds;
    return 0;
}

uid_t getuid(void) {
    return 0;
}

gid_t getgid(void) {
    return 0;
}

int dup2(int oldfd, int newfd) {
    long ret = myos_syscall3(MYOS_SYS_DUP2, oldfd, newfd, 0);
    if (ret == (long)MYOS_SYSERR) {
        errno = EBADF;
        return -1;
    }
    return newfd;
}

/*
 * F_DUPFD: kernel MAX_FDS is 16 and oksh FDBASE is 10. dup2 always clobbers, so
 * bump from max(arg, 10) to hand out distinct high fds. F_GETFL/F_SETFD succeed.
 * Named _fcntl so it does not clash with newlib libc's ENOSYS fcntl().
 */
int _fcntl(int fd, int cmd, int arg) {
    static int next_highfd = 10;

    switch (cmd) {
#ifdef F_DUPFD_CLOEXEC
    case F_DUPFD_CLOEXEC:
#endif
    case F_DUPFD: {
        int minfd = arg;
        int n;

        if (minfd < 10) {
            minfd = 10;
        }
        if (minfd < next_highfd) {
            minfd = next_highfd;
        }
        for (n = minfd; n < 16; n++) {
            if (n == fd) {
                continue;
            }
            if (dup2(fd, n) >= 0) {
                next_highfd = n + 1;
                return n;
            }
        }
        errno = EMFILE;
        return -1;
    }
    case F_GETFL:
        (void)arg;
        return 0;
    case F_SETFL:
        (void)arg;
        return 0;
    case F_GETFD:
        (void)arg;
        return 0;
    case F_SETFD:
        (void)arg;
        return 0;
    default:
        (void)fd;
        errno = EINVAL;
        return -1;
    }
}

int fchownat(int dirfd, const char *path, uid_t owner, gid_t group, int flags) {
    (void)dirfd;
    (void)path;
    (void)owner;
    (void)group;
    (void)flags;
    return myos_rofs();
}

int symlinkat(const char *target, int dirfd, const char *path) {
    (void)target;
    (void)dirfd;
    (void)path;
    return myos_rofs();
}

int setpriority(int which, id_t who, int prio) {
    (void)which;
    (void)who;
    (void)prio;
    return myos_nosys();
}

int setsid(void) {
    return myos_nosys();
}

struct group *getgrnam(const char *name) {
    (void)name;
    errno = ENOENT;
    return NULL;
}


int _mkdir(const char *path, mode_t mode) {
    return mkdir(path, mode);
}

int _chmod(const char *path, mode_t mode) {
    return chmod(path, mode);
}

int _access(const char *path, int mode) {
    return access(path, mode);
}

int _creat(const char *path, mode_t mode) {
    return creat(path, mode);
}

mode_t _umask(mode_t mask) {
    return umask(mask);
}

int _symlink(const char *target, const char *linkpath) {
    return symlink(target, linkpath);
}
