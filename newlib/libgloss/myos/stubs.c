/* myos libgloss: POSIX hooks with no kernel backing yet (errno = ENOSYS). */

#include <_ansi.h>
#include <errno.h>
#include <reent.h>
#include <stdint.h>
#include <sys/stat.h>
#include <sys/time.h>
#include <sys/times.h>
#include <unistd.h>

#include "myos_syscalls.h"

int _lseek(int fd, off_t pos, int whence) {
    (void)fd;
    (void)pos;
    (void)whence;
    errno = ENOSYS;
    return -1;
}

int _unlink(const char *path) {
    (void)path;
    errno = EROFS;
    return -1;
}

int _link(const char *oldpath, const char *newpath) {
    (void)oldpath;
    (void)newpath;
    errno = EROFS;
    return -1;
}

int _kill(int pid, int sig) {
    (void)pid;
    (void)sig;
    errno = ENOSYS;
    return -1;
}

int _fork(void) {
    long ret = myos_syscall0(MYOS_SYS_FORK);
    if (ret == (long)MYOS_SYSERR) {
        errno = EAGAIN;
        return -1;
    }
    return (int)ret;
}

int _wait(int *status) {
    long ret = myos_syscall1(MYOS_SYS_WAIT, (long)(uintptr_t)status);
    if (ret == (long)MYOS_SYSERR) {
        errno = ECHILD;
        return -1;
    }
    return (int)ret;
}

int _execve(const char *path, char *const argv[], char *const envp[]) {
    (void)path;
    (void)argv;
    (void)envp;
    errno = ENOSYS;
    return -1;
}

clock_t _times(struct tms *buf) {
    (void)buf;
    errno = ENOSYS;
    return (clock_t)-1;
}

int _gettimeofday(struct timeval *tv, void *tz) {
    (void)tv;
    (void)tz;
    errno = ENOSYS;
    return -1;
}

int _chown(const char *path, uid_t owner, gid_t group) {
    (void)path;
    (void)owner;
    (void)group;
    errno = EROFS;
    return -1;
}
