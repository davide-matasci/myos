/* myos libgloss: POSIX helpers sbase expects beyond read-only VFS hooks. */

#include <dirent.h>
#include <errno.h>
#include <fcntl.h>
#include <sys/wait.h>
#include <grp.h>
#include <pwd.h>
#include <stdarg.h>
#include <stdlib.h>
#include <stdint.h>
#include <string.h>
#include <sys/stat.h>
#include <sys/time.h>
#include <unistd.h>
#include <utime.h>
#include <sys/myos_extra.h> /* lstat */

#include "myos_syscalls.h"

static int myos_rofs(void) {
    errno = EROFS;
    return -1;
}

static int myos_nosys(void) {
    errno = ENOSYS;
    return -1;
}

int creat(const char *path, mode_t mode) {
    (void)mode;
    return open(path, O_WRONLY | O_CREAT | O_TRUNC, 0666);
}

int chmod(const char *path, mode_t mode) {
    (void)path;
    (void)mode;
    /* No mode bits in VFS yet; succeed so git config lock chmod works. */
    return 0;
}

mode_t umask(mode_t mask) {
    static mode_t cur = 022;
    mode_t old = cur;
    cur = mask;
    return old;
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

/* timeval pair (NULL = both now) -> timespec pair. */
static const struct timespec *myos_timespec_from_timeval(const struct timeval tv[2],
                                                         struct timespec ts[2]) {
    if (tv == NULL) {
        return NULL;
    }
    for (int i = 0; i < 2; i++) {
        ts[i].tv_sec = tv[i].tv_sec;
        ts[i].tv_nsec = tv[i].tv_usec * 1000L;
    }
    return ts;
}

int utimes(const char *path, const struct timeval times[2]) {
    struct timespec ts[2];
    return utimensat(AT_FDCWD, path, myos_timespec_from_timeval(times, ts), 0);
}

int lutimes(const char *path, const struct timeval times[2]) {
    struct timespec ts[2];
    return utimensat(AT_FDCWD, path, myos_timespec_from_timeval(times, ts), AT_SYMLINK_NOFOLLOW);
}

int futimes(int fd, const struct timeval times[2]) {
    struct timespec ts[2];
    return futimens(fd, myos_timespec_from_timeval(times, ts));
}

int utime(const char *path, const struct utimbuf *times) {
    struct timespec ts[2];
    if (times == NULL) {
        return utimensat(AT_FDCWD, path, NULL, 0);
    }
    ts[0].tv_sec = times->actime;
    ts[0].tv_nsec = 0;
    ts[1].tv_sec = times->modtime;
    ts[1].tv_nsec = 0;
    return utimensat(AT_FDCWD, path, ts, 0);
}

extern char **environ;

int execvp(const char *file, char *const argv[]) {
    const char *path;
    char cand[512];
    const char *p, *end;

    if (file == NULL) {
        errno = ENOENT;
        return -1;
    }
    /* Getty/login pass absolute paths (/u/login, /sh); absolute/contain-slash
     * names go straight to execve. Everything else gets a POSIX PATH search
     * (GNU make execvp()s bare recipe commands like `echo`). */
    if (file[0] == '/') {
        return execve(file, argv, environ);
    }
    if (strchr(file, '/') != NULL) {
        return execve(file, argv, environ);
    }
    path = getenv("PATH");
    if (path == NULL) {
        path = "/bin/sbase:/bin/coreutils:/bin/ubase:/bin/custom:/bin/tcc:/bin/std:/bin/etc";
    }
    for (p = path; *p != '\0'; p = (*end == ':') ? end + 1 : end) {
        end = strchr(p, ':');
        if (end == NULL) {
            end = p + strlen(p);
        }
        size_t dirlen = (size_t)(end - p);
        if (dirlen == 0) {
            /* POSIX: empty PATH entry means the current directory. */
            if (1 + strlen(file) + 1 > sizeof(cand)) {
                continue;
            }
            cand[0] = '.';
            cand[1] = '/';
            memcpy(cand + 2, file, strlen(file) + 1);
            execve(cand, argv, environ);
            if (errno != ENOENT && errno != ENOTDIR && errno != EACCES) {
                return -1;
            }
            continue;
        }
        if (dirlen + 1 + strlen(file) + 1 > sizeof(cand)) {
            continue;
        }
        memcpy(cand, p, dirlen);
        if (dirlen > 0 && cand[dirlen - 1] != '/') {
            cand[dirlen] = '/';
            dirlen++;
        }
        memcpy(cand + dirlen, file, strlen(file) + 1);
        execve(cand, argv, environ);
        if (errno != ENOENT && errno != ENOTDIR && errno != EACCES) {
            return -1;
        }
    }
    errno = ENOENT;
    return -1;
}

#ifndef WNOHANG
#define WNOHANG 1
#endif

/*
 * SYS_WAITPID(status, options, pid): the kernel writes a POSIX int status
 * (WIFSIGNALED for a signal death) and waits for `pid` when it is > 0, for
 * any child otherwise (process-group waits are treated as "any"). WNOHANG
 * keeps dropbear's `while (waitpid(-1, &st, WNOHANG) > 0)` reap loop from
 * blocking after the last zombie. WUNTRACED is ignored: nothing stops.
 */
pid_t waitpid(pid_t pid, int *status, int options) {
    int st = 0;
    long ret;
    long opts = 0;

    if ((options & WNOHANG) != 0) {
        opts |= MYOS_WAIT_NOHANG;
    }

    ret = myos_syscall3(MYOS_SYS_WAITPID, status ? (long)(uintptr_t)&st : 0, opts,
                        pid > 0 ? (long)pid : 0);

    if (ret == (long)MYOS_EINTR) {
        errno = EINTR;
        return -1;
    }
    if (ret == (long)MYOS_SYSERR) {
        errno = ECHILD;
        return -1;
    }
    /* Kernel returns 0 for WNOHANG with live children but no zombie. */
    if (ret == 0 && (options & WNOHANG) != 0) {
        return 0;
    }
    if (status != NULL) {
        *status = st;
    }
    return (pid_t)ret;
}

long sysconf(int name) {
#ifdef _SC_OPEN_MAX
    if (name == _SC_OPEN_MAX) {
        return MYOS_OPEN_MAX;
    }
#endif
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
#ifdef _SC_GETPW_R_SIZE_MAX
    if (name == _SC_GETPW_R_SIZE_MAX) {
        return 1024;
    }
#endif
#ifdef _SC_GETGR_R_SIZE_MAX
    if (name == _SC_GETGR_R_SIZE_MAX) {
        return 1024;
    }
#endif
    /* Common numeric values if headers did not expose _SC_PAGESIZE. */
    if (name == 8 || name == 11 || name == 30 || name == 39) {
        return 4096;
    }
    /* newlib: _SC_GETGR_R_SIZE_MAX=50, _SC_GETPW_R_SIZE_MAX=51 */
    if (name == 50 || name == 51) {
        return 1024;
    }
    (void)name;
    errno = ENOSYS;
    return -1;
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


/* F_GETLK, F_SETLK, F_SETLKW: newlib's struct flock to the kernel's
 * lock range (from l_whence to an absolute start, a negative l_len to the
 * bytes before l_start) and back for F_GETLK. */
static int myos_record_lock(int fd, int cmd, struct flock *fl) {
    struct myos_lock_range r = {0};
    long long start = fl->l_start;
    long long len = fl->l_len;
    long ret;

    switch (fl->l_type) {
    case F_RDLCK: r.kind = MYOS_LOCK_SHARED; break;
    case F_WRLCK: r.kind = MYOS_LOCK_EXCLUSIVE; break;
    case F_UNLCK: r.kind = MYOS_LOCK_UNLOCK; break;
    default: errno = EINVAL; return -1;
    }
    if (fl->l_whence == SEEK_CUR) {
        off_t pos = lseek(fd, 0, SEEK_CUR);
        if (pos < 0) {
            return -1;
        }
        start += pos;
    } else if (fl->l_whence == SEEK_END) {
        struct stat st;
        if (fstat(fd, &st) < 0) {
            return -1;
        }
        start += st.st_size;
    } else if (fl->l_whence != SEEK_SET) {
        errno = EINVAL;
        return -1;
    }
    if (len < 0) {
        start += len;
        len = -len;
    }
    if (start < 0) {
        errno = EINVAL;
        return -1;
    }
    r.start = (unsigned long long)start;
    r.len = (unsigned long long)len;
    ret = myos_syscall3(MYOS_SYS_LOCKCTL, fd,
                        cmd == F_GETLK ? MYOS_LOCKCTL_GET : cmd == F_SETLK ? MYOS_LOCKCTL_SET : MYOS_LOCKCTL_WAIT,
                        (long)&r);
    if (ret == (long)MYOS_EAGAIN) {
        errno = EAGAIN;
        return -1;
    }
    if (ret == (long)MYOS_EINTR) {
        errno = EINTR;
        return -1;
    }
    if (ret == (long)MYOS_SYSERR) {
        errno = EBADF;
        return -1;
    }
    if (cmd == F_GETLK) {
        fl->l_type = r.kind == MYOS_LOCK_SHARED ? F_RDLCK : r.kind == MYOS_LOCK_EXCLUSIVE ? F_WRLCK : F_UNLCK;
        if (r.kind != MYOS_LOCK_UNLOCK) {
            fl->l_whence = SEEK_SET;
            fl->l_start = (off_t)r.start;
            fl->l_len = (off_t)r.len;
            fl->l_pid = (short)r.pid;
        }
    }
    return 0;
}

/* newlib libc exports fcntl() when HAVE_FCNTL; we supply the syscall glue.
 * `wide` is the whole argument word (a pointer for the lock commands,
 * toolchain/newlib/patch.sh patch_fcntl_arg); the others take an int. */
int _fcntl(int fd, int cmd, long wide) {
    int arg = (int)wide;

    if (cmd == F_GETLK || cmd == F_SETLK || cmd == F_SETLKW) {
        return myos_record_lock(fd, cmd, (struct flock *)wide);
    }
    if (cmd == F_DUPFD
#ifdef F_DUPFD_CLOEXEC
        || cmd == F_DUPFD_CLOEXEC
#endif
    ) {
        long flags = 0;
#ifdef F_DUPFD_CLOEXEC
        if (cmd == F_DUPFD_CLOEXEC) {
            flags = MYOS_FD_CLOEXEC;
        }
#endif
        long ret = myos_syscall3(MYOS_SYS_DUPFD, fd, arg, flags);
        if (ret == (long)MYOS_SYSERR) {
            errno = EBADF;
            return -1;
        }
        myos_fd_nonblock_dup(fd, (int)ret);
        return (int)ret;
    }

    switch (cmd) {
    case F_GETFD:
    case F_SETFD: {
        long ret = cmd == F_GETFD ? myos_syscall3(MYOS_SYS_FDFLAGS, fd, MYOS_FD_GET, 0)
                                  : myos_syscall3(MYOS_SYS_FDFLAGS, fd, MYOS_FD_SET,
                                                  (arg & FD_CLOEXEC) ? MYOS_FD_CLOEXEC : 0);
        if (ret == (long)MYOS_SYSERR) {
            errno = EBADF;
            return -1;
        }
        return cmd == F_GETFD ? ((ret & MYOS_FD_CLOEXEC) ? FD_CLOEXEC : 0) : 0;
    }
    case F_GETFL: {
        int sockfl = myos_socket_fcntl(fd, F_GETFL, 0);
        if (sockfl >= 0) {
            return sockfl;
        }
        (void)arg;
        return O_RDWR | (myos_fd_nonblock_get(fd) ? O_NONBLOCK : 0);
    }
    case F_SETFL: {
        int sockfl = myos_socket_fcntl(fd, F_SETFL, arg);
        if (sockfl == 0) {
            return 0;
        }
        if (sockfl == -2) {
            return -1;
        }
        /* Non-socket (pipes): track O_NONBLOCK in userspace. */
        myos_fd_nonblock_set(fd, (arg & O_NONBLOCK) != 0);
        return 0;
    }
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
    /* TEMP bisect (revert): SYS_SETSID corrupts the netfs write path after
     * setsid — kernel page fault reproducible via tcp_fork_smoke with
     * setsid + accepted fd >= 5, and dropbear's banner write fails with
     * EIO. Until root-caused, only the process group half: the caller leads
     * a new group (a no-op for a group leader), so a pty's hangup and ^C,
     * sent to its claimant's group, stay with what the caller starts (st's
     * shell, a forkpty child) instead of reaching its parent's group, the
     * login session and init. Returns pid as the sid. */
    (void)setpgid(0, 0);
    return (int)getpid();
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

int _chmod(const char *path, mode_t mode) {
    return chmod(path, mode);
}

int _creat(const char *path, mode_t mode) {
    return creat(path, mode);
}

mode_t _umask(mode_t mask) {
    return umask(mask);
}

