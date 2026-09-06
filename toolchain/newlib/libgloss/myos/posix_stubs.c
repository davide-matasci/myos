/* myos libgloss: POSIX helpers sbase expects beyond read-only VFS hooks. */

#include <dirent.h>
#include <errno.h>
#include <fcntl.h>
#include <grp.h>
#include <pwd.h>
#include <stdarg.h>
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
    (void)mode;
    return open(path, O_WRONLY | O_CREAT | O_TRUNC, 0666);
}

int chmod(const char *path, mode_t mode) {
    (void)path;
    (void)mode;
    return myos_rofs();
}

int mkdir(const char *path, mode_t mode) {
    long ret;
    if (path == NULL) {
        errno = ENOENT;
        return -1;
    }
    ret = myos_syscall3(
        MYOS_SYS_MKDIR, (long)(uintptr_t)path, (long)strlen(path), (long)mode);
    if (ret == (long)MYOS_SYSERR) {
        errno = EROFS;
        return -1;
    }
    return 0;
}

mode_t umask(mode_t mask) {
    static mode_t cur = 022;
    mode_t old = cur;
    cur = mask;
    return old;
}

int symlink(const char *target, const char *linkpath) {
    size_t tlen;
    size_t llen;
    long packed;
    long ret;

    if (target == NULL || linkpath == NULL) {
        errno = ENOENT;
        return -1;
    }
    tlen = strlen(target);
    llen = strlen(linkpath);
    if (tlen == 0 || llen == 0 || tlen > 0xffff || llen > 0xffff) {
        errno = ENAMETOOLONG;
        return -1;
    }
    packed = (long)((tlen << 16) | llen);
    ret = myos_syscall3(
        MYOS_SYS_SYMLINK,
        (long)(uintptr_t)target,
        (long)(uintptr_t)linkpath,
        packed);
    if (ret == (long)MYOS_SYSERR) {
        errno = EROFS;
        return -1;
    }
    return 0;
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

#ifndef AT_FDCWD
#define AT_FDCWD (-100)
#endif

int openat(int dirfd, const char *path, int flags, ...) {
    char full[512];
    mode_t mode = 0;
    va_list ap;

    if (path == NULL) {
        errno = ENOENT;
        return -1;
    }
    if (myos_fd_path_resolve(dirfd, path, full, sizeof full) < 0) {
        return -1;
    }
    if (flags & O_CREAT) {
        va_start(ap, flags);
        mode = (mode_t)va_arg(ap, int);
        va_end(ap);
        return open(full, flags, mode);
    }
    return open(full, flags);
}

int faccessat(int dirfd, const char *path, int mode, int flags) {
    char full[512];
    (void)flags;
    if (myos_fd_path_resolve(dirfd, path, full, sizeof full) < 0) {
        return -1;
    }
    return access(full, mode);
}

int fstatat(int dirfd, const char *path, struct stat *st, int flags) {
    char full[512];
    (void)flags;
    if (st == NULL) {
        errno = EINVAL;
        return -1;
    }
    if (myos_fd_path_resolve(dirfd, path, full, sizeof full) < 0) {
        return -1;
    }
    return stat(full, st);
}

#ifndef AT_REMOVEDIR
#define AT_REMOVEDIR 0x200
#endif

int unlinkat(int dirfd, const char *path, int flags) {
    char full[512];
    if (path == NULL) {
        errno = ENOENT;
        return -1;
    }
    if (myos_fd_path_resolve(dirfd, path, full, sizeof full) < 0) {
        return -1;
    }
    if (flags & AT_REMOVEDIR) {
        return rmdir(full);
    }
    return unlink(full);
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

extern char **environ;

int execvp(const char *file, char *const argv[]) {
    if (file == NULL) {
        errno = ENOENT;
        return -1;
    }
    /* Getty/login pass absolute paths (/u/login, /sh); skip PATH search. */
    return execve(file, argv, environ);
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
#ifdef _SC_PAGESIZE
    if (name == _SC_PAGESIZE) {
        return 4096;
    }
#endif
#ifdef _SC_PAGE_SIZE
    if (name == _SC_PAGE_SIZE) {
        return 4096;
    }
#endif
    /* Common numeric values if headers did not expose _SC_PAGESIZE. */
    if (name == 8 || name == 11 || name == 30 || name == 39) {
        return 4096;
    }
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

int setuid(uid_t u) {
    (void)u;
    return 0;
}

int seteuid(uid_t u) {
    (void)u;
    return 0;
}

int setgid(gid_t g) {
    (void)g;
    return 0;
}

int setegid(gid_t g) {
    (void)g;
    return 0;
}

int setgroups(int n, const gid_t *l) {
    (void)n;
    (void)l;
    return 0;
}

gid_t getegid(void) {
    return 0;
}


int dup2(int oldfd, int newfd) {
    long ret = myos_syscall3(MYOS_SYS_DUP2, oldfd, newfd, 0);
    if (ret == (long)MYOS_SYSERR) {
        errno = EBADF;
        return -1;
    }
    myos_fd_dup_tty(oldfd, newfd);
    myos_fd_path_dup(oldfd, newfd);
    return newfd;
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
    char full[512];
    if (path == NULL) {
        errno = ENOENT;
        return -1;
    }
    if (myos_fd_path_resolve(dirfd, path, full, sizeof full) < 0) {
        return -1;
    }
    return symlink(target, full);
}


/* newlib libc exports fcntl() when HAVE_FCNTL; we supply the syscall glue. */
int _fcntl(int fd, int cmd, int arg) {
    if (cmd == F_DUPFD
#ifdef F_DUPFD_CLOEXEC
        || cmd == F_DUPFD_CLOEXEC
#endif
    ) {
        long ret = myos_syscall3(MYOS_SYS_DUPFD, fd, arg, 0);
        if (ret == (long)MYOS_SYSERR) {
            errno = EBADF;
            return -1;
        }
        myos_fd_dup_tty(fd, (int)ret);
        myos_fd_path_dup(fd, (int)ret);
        return (int)ret;
    }

    switch (cmd) {
    case F_GETFD:
        /* Validity only; CLOEXEC not tracked. Accept stdio + shell FDBASE range. */
        if (fd < 0 || fd >= 16) {
            errno = EBADF;
            return -1;
        }
        return 0;
    case F_SETFD:
        (void)fd;
        (void)arg;
        return 0;
    case F_GETFL:
        (void)fd;
        (void)arg;
        return O_RDWR;
    case F_SETFL:
        (void)fd;
        (void)arg;
        return 0;
    default:
        (void)fd;
        (void)arg;
        errno = EINVAL;
        return -1;
    }
}

int setpriority(int which, id_t who, int prio) {
    (void)which;
    (void)who;
    (void)prio;
    return myos_nosys();
}

int setsid(void) {
    /* SYS_SETSID: become session leader (sid = pid / task slot), join a new
     * process group (pgid = pid), and clear the controlling tty. Kernel
     * returns the new sid, or SYSERR if already a session leader (EPERM). */
    long ret = myos_syscall0(MYOS_SYS_SETSID);
    if (ret == (long)MYOS_SYSERR) {
        errno = EPERM;
        return -1;
    }
    return (int)ret;
}

int setpgid(pid_t pid, pid_t pgid) {
    /* SYS_SETPGID: move pid (0 = self) into process group pgid (0 = create
     * group with the target's pid). Phase-1: same session; self or direct
     * child only; new pgid must be target pid or an existing group in the
     * session. */
    long ret = myos_syscall3(MYOS_SYS_SETPGID, (long)pid, (long)pgid, 0);
    if (ret == (long)MYOS_SYSERR) {
        errno = EPERM;
        return -1;
    }
    return 0;
}

pid_t getpgid(pid_t pid) {
    long ret = myos_syscall1(MYOS_SYS_GETPGID, (long)pid);
    if (ret == (long)MYOS_SYSERR) {
        errno = ESRCH;
        return (pid_t)-1;
    }
    return (pid_t)ret;
}

pid_t getsid(pid_t pid) {
    long ret = myos_syscall1(MYOS_SYS_GETSID, (long)pid);
    if (ret == (long)MYOS_SYSERR) {
        errno = ESRCH;
        return (pid_t)-1;
    }
    return (pid_t)ret;
}

/* getpwnam/getgrnam live in pwdgrp.c (root:root only). */

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
