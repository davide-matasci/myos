/* myos libgloss: POSIX hooks with no kernel backing yet (errno = ENOSYS). */

#include <_ansi.h>
#include <errno.h>
#include <reent.h>
#include <stdint.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>
#include <sys/time.h>
#include <sys/times.h>
#include <unistd.h>

#include "myos_syscalls.h"

int _lseek(int fd, off_t pos, int whence) {
    long ret = myos_syscall3(MYOS_SYS_LSEEK, fd, (long)pos, whence);
    if (ret == (long)MYOS_SYSERR) {
        errno = ESPIPE;
        return -1;
    }
    return (int)ret;
}


/* myos has no hard links: a file has one name (docs/security.md). EPERM is
 * what Linux says for a filesystem without them; git then renames its
 * objects into place instead. */
int _link(const char *oldpath, const char *newpath) {
    (void)oldpath;
    (void)newpath;
    errno = EPERM;
    return -1;
}

int _kill(int pid, int sig) {
    /* Signal numbers must match newlib <signal.h> / kernel signal.rs; 0
     * only checks that the target exists. */
    if (sig < 0 || sig > 31) {
        errno = EINVAL;
        return -1;
    }
    long ret = myos_syscall3(MYOS_SYS_KILL, (long)pid, (long)sig, 0);
    if (ret == (long)MYOS_SYSERR) {
        errno = ESRCH;
        return -1;
    }
    return 0;
}

/* pthread.c: the pthread_atfork handlers, and the child's thread state. */
void __myos_fork_prepare(void);
void __myos_fork_done(int child);

int _fork(void) {
    __myos_fork_prepare();
    long ret = myos_syscall0(MYOS_SYS_FORK);
    __myos_fork_done(ret == 0);
    if (ret == (long)MYOS_SYSERR) {
        errno = EAGAIN;
        return -1;
    }
    return (int)ret;
}

int _wait(int *status) {
    int st = 0;
    long ret = myos_syscall3(MYOS_SYS_WAITPID, (long)(uintptr_t)&st, 0, 0);
    if (ret == (long)MYOS_EINTR) {
        errno = EINTR;
        return -1;
    }
    if (ret == (long)MYOS_SYSERR) {
        errno = ECHILD;
        return -1;
    }
    if (status != NULL) {
        *status = st;
    }
    return (int)ret;
}

/*
 * Pack argv/envp into the myos SYS_EXEC layout:
 *   [argc, (ptr,len)…, envc, (ptr,len)…]
 * Strings are copied into one heap block so the kernel copy-in sees writable
 * user memory (AArch64 ET_EXEC rodata VAs are rejected). The limits match the
 * kernel's (kernel/src/user/mod.rs): MAX_ARGC, MAX_ENVC, and MAX_EXEC_STRINGS
 * bytes for all strings with their NULs.
 */
#define MYOS_MAX_ARGC 1024
#define MYOS_MAX_ENVC 1024
#define MYOS_MAX_EXEC_STRINGS (128 * 1024)
/* Matches the kernel exec path limit (kernel/src/user/mod.rs MAX_PATH). */
#define MYOS_MAX_PATH 256

clock_t _times(struct tms *buf) {
    (void)buf;
    errno = ENOSYS;
    return (clock_t)-1;
}

int _gettimeofday(struct timeval *tv, void *tz) {
    (void)tz;
    if (tv == NULL) {
        errno = EINVAL;
        return -1;
    }
    long ret = myos_syscall3(
        MYOS_SYS_GETTIMEOFDAY, (long)(uintptr_t)tv, 0, 0);
    if (ret == (long)MYOS_SYSERR) {
        errno = EIO;
        return -1;
    }
    return 0;
}

int _chown(const char *path, uid_t owner, gid_t group) {
    (void)path;
    (void)owner;
    (void)group;
    errno = EROFS;
    return -1;
}


