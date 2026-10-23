/* The process, fd-table and host calls over the kernel's: dup, fcntl (its
 * record locks), pipe, the sessions and process groups, the host name,
 * the interval timer, the terminal's name; and the calls myos answers
 * without a kernel feature behind them: the mode and owner bits (every
 * process is root and the VFS keeps no modes), hard links (none:
 * docs/security.md), truncation. fcntl, link, pipe, chmod, creat and
 * umask have no alias: newlib's libc defines them over the hooks of
 * libgloss/myos/hooks.c. */

#include <errno.h>
#include <fcntl.h>
#include <signal.h>
#include <stdint.h>
#include <stdlib.h>
#include <string.h>
#include <sys/file.h>
#include <sys/stat.h>
#include <sys/time.h>
#include <sys/utsname.h>
#include <unistd.h>

#include "myos_syscalls.h"
#include "libc.h"

static int myos_rofs(void) {
    errno = EROFS;
    return -1;
}

static int myos_nosys(void) {
    errno = ENOSYS;
    return -1;
}

/* ---- the fd table ---------------------------------------------------- */

int __myos_dup2(int oldfd, int newfd) {
    long ret = myos_syscall3(MYOS_SYS_DUPFD, oldfd, newfd, MYOS_DUP_EXACT);
    if (ret == (long)MYOS_SYSERR) {
        errno = EBADF;
        return -1;
    }
    return newfd;
}

/* F_GETLK, F_SETLK, F_SETLKW: struct flock to the kernel's lock range
 * (from l_whence to an absolute start, a negative l_len to the bytes
 * before l_start) and back for F_GETLK. */
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
            fl->l_pid = (pid_t)r.pid;
        }
    }
    return 0;
}

/* fcntl's syscall glue. `wide` is the whole argument word (a pointer for
 * the lock commands); the others take an int. */
int __myos_fcntl(int fd, int cmd, long wide) {
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

int __myos_pipe(int fildes[2]) {
    return myos_pipe_flags(fildes, 0);
}

int __myos_flock(int fd, int operation) {
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

int __myos_ftruncate(int fd, off_t length) {
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

/* ---- names, modes and owners ------------------------------------------ */

/* myos has no hard links: a file has one name (docs/security.md). EPERM is
 * what Linux says for a filesystem without them; git then renames its
 * objects into place instead. */
int __myos_link(const char *oldpath, const char *newpath) {
    (void)oldpath;
    (void)newpath;
    errno = EPERM;
    return -1;
}

int __myos_linkat(int olddirfd, const char *oldpath, int newdirfd, const char *newpath, int flags) {
    (void)olddirfd;
    (void)oldpath;
    (void)newdirfd;
    (void)newpath;
    (void)flags;
    errno = EPERM;
    return -1;
}

int __myos_chmod(const char *path, mode_t mode) {
    (void)path;
    (void)mode;
    /* No mode bits in VFS yet; succeed so git config lock chmod works. */
    return 0;
}

int __myos_fchmod(int fd, mode_t mode) {
    (void)fd;
    (void)mode;
    return 0;
}

int __myos_fchmodat(int dirfd, const char *path, mode_t mode, int flags) {
    (void)dirfd;
    (void)path;
    (void)mode;
    (void)flags;
    /* Match chmod/fchmod: no mode bits yet. */
    return 0;
}

int __myos_chown(const char *path, uid_t owner, gid_t group) {
    (void)path;
    (void)owner;
    (void)group;
    return myos_rofs();
}

int __myos_lchown(const char *path, uid_t owner, gid_t group) {
    (void)path;
    (void)owner;
    (void)group;
    return myos_rofs();
}

int __myos_fchown(int fd, uid_t owner, gid_t group) {
    (void)fd;
    (void)owner;
    (void)group;
    /* Always root; getty fchown on the console tty. */
    return 0;
}

int __myos_fchownat(int dirfd, const char *path, uid_t owner, gid_t group, int flags) {
    (void)dirfd;
    (void)path;
    (void)owner;
    (void)group;
    (void)flags;
    return myos_rofs();
}

/* ---- sessions and process groups -------------------------------------- */

int __myos_setsid(void) {
    /* SYS_SETSID: a new session and process group led by the caller, no
     * controlling terminal; EPERM when it already leads a group. */
    long ret = myos_syscall3(MYOS_SYS_SETSID, 0, 0, 0);
    if (ret == (long)MYOS_SYSERR) {
        errno = EPERM;
        return -1;
    }
    return (int)ret;
}

int __myos_setpgid(pid_t pid, pid_t pgid) {
    /* SYS_SETPGID: move pid (0 = self) into process group pgid (0 = create
     * group with the target's pid). Same session; self or a child that has
     * not exec'd (EACCES); new pgid must be target pid or an existing group
     * in the session. */
    long ret = myos_syscall3(MYOS_SYS_SETPGID, (long)pid, (long)pgid, 0);
    if (ret == (long)MYOS_EACCES) {
        errno = EACCES;
        return -1;
    }
    if (ret == (long)MYOS_SYSERR) {
        errno = EPERM;
        return -1;
    }
    return 0;
}

pid_t __myos_getpgid(pid_t pid) {
    long ret = myos_syscall1(MYOS_SYS_GETPGID, (long)pid);
    if (ret == (long)MYOS_SYSERR) {
        errno = ESRCH;
        return (pid_t)-1;
    }
    return (pid_t)ret;
}

pid_t __myos_getsid(pid_t pid) {
    long ret = myos_syscall1(MYOS_SYS_GETSID, (long)pid);
    if (ret == (long)MYOS_SYSERR) {
        errno = ESRCH;
        return (pid_t)-1;
    }
    return (pid_t)ret;
}

pid_t __myos_getppid(void) {
    return (pid_t)myos_syscall0(MYOS_SYS_GETPPID);
}

/* ---- the host ---------------------------------------------------------- */

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

int __myos_gethostname(char *name, size_t len) {
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

int __myos_sethostname(const char *name, size_t len) {
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

int __myos_uname(struct utsname *buf) {
    if (buf == NULL) {
        errno = EFAULT;
        return -1;
    }
    strncpy(buf->sysname, "myos", sizeof(buf->sysname));
    if (__myos_gethostname(buf->nodename, sizeof(buf->nodename)) < 0) {
        strncpy(buf->nodename, "myos", sizeof(buf->nodename));
    }
    strncpy(buf->release, "0.1", sizeof(buf->release));
    strncpy(buf->version, "myos", sizeof(buf->version));
    strncpy(buf->machine, "myos", sizeof(buf->machine));
    return 0;
}

/* ---- the interval timer ------------------------------------------------ */

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

int __myos_setitimer(int which, const struct itimerval *value, struct itimerval *old) {
    if (!value) {
        errno = EFAULT;
        return -1;
    }
    return itimer(which, value, old);
}

int __myos_getitimer(int which, struct itimerval *value) {
    if (!value) {
        errno = EFAULT;
        return -1;
    }
    return itimer(which, NULL, value);
}

/* ---- the terminal's name ------------------------------------------------ */

int __myos_ttyname_r(int fd, char *buf, size_t len) {
    char name[MYOS_TTY_PATH];
    int master;

    if (myos_tty_dir(fd, name, sizeof name - 5, &master) != 0 || master) {
        return ENOTTY;
    }
    strcat(name, "/data");
    if (strlen(name) >= len) {
        return ERANGE;
    }
    strcpy(buf, name);
    return 0;
}

char *__myos_ttyname(int fd) {
    static char name[MYOS_TTY_PATH];
    int rc = __myos_ttyname_r(fd, name, sizeof name);

    if (rc != 0) {
        errno = rc;
        return NULL;
    }
    return name;
}

MYOS_ALIAS(dup2)
MYOS_ALIAS(pipe)
MYOS_ALIAS(chmod)
MYOS_ALIAS(flock)
MYOS_ALIAS(ftruncate)
MYOS_ALIAS(linkat)
MYOS_ALIAS(fchmod)
MYOS_ALIAS(fchmodat)
MYOS_ALIAS(chown)
MYOS_ALIAS(lchown)
MYOS_ALIAS(fchown)
MYOS_ALIAS(fchownat)
MYOS_ALIAS(setsid)
MYOS_ALIAS(setpgid)
MYOS_ALIAS(getpgid)
MYOS_ALIAS(getsid)
MYOS_ALIAS(getppid)
MYOS_ALIAS(gethostname)
MYOS_ALIAS(sethostname)
MYOS_ALIAS(uname)
MYOS_ALIAS(setitimer)
MYOS_ALIAS(getitimer)
MYOS_ALIAS(ttyname_r)
MYOS_ALIAS(ttyname)
