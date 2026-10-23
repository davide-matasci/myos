/* myos libgloss: the POSIX calls newlib's libc lacks that are only libc
 * (over the calls of toolchain/libmyos): creat, umask, the utimes family,
 * execvp, sysconf, the user and group ids. */

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

static int myos_nosys(void) {
    errno = ENOSYS;
    return -1;
}

int creat(const char *path, mode_t mode) {
    (void)mode;
    return open(path, O_WRONLY | O_CREAT | O_TRUNC, 0666);
}

mode_t umask(mode_t mask) {
    static mode_t cur = 022;
    mode_t old = cur;
    cur = mask;
    return old;
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



int setpriority(int which, id_t who, int prio) {
    (void)which;
    (void)who;
    (void)prio;
    return myos_nosys();
}

int _creat(const char *path, mode_t mode) {
    return creat(path, mode);
}

mode_t _umask(mode_t mask) {
    return umask(mask);
}
