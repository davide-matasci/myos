/* Runtime stubs / thin wrappers for Git on myos. */
#include <errno.h>
#include <fcntl.h>
#include <netdb.h>
#include <stdarg.h>
#include <stdint.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>
#include <sys/statvfs.h>
#include <sys/time.h>
#include <sys/types.h>
#include <sys/utime.h>
#include <unistd.h>

#ifndef AT_FDCWD
#define AT_FDCWD (-100)
#endif

extern char **environ;

int pipe2(int pipefd[2], int flags) {
    (void)flags;
    return pipe(pipefd);
}

int prctl(int option, ...) {
    (void)option;
    errno = ENOSYS;
    return -1;
}

int sync_file_range(int fd, off_t offset, off_t nbytes, unsigned int flags) {
    (void)fd;
    (void)offset;
    (void)nbytes;
    (void)flags;
    return 0;
}

int statvfs(const char *path, struct statvfs *buf) {
    (void)path;
    if (!buf) {
        errno = EFAULT;
        return -1;
    }
    buf->f_bsize = 4096;
    buf->f_frsize = 4096;
    buf->f_blocks = 1UL << 20;
    buf->f_bfree = 1UL << 19;
    buf->f_bavail = 1UL << 19;
    buf->f_files = 1UL << 16;
    buf->f_ffree = 1UL << 15;
    buf->f_favail = 1UL << 15;
    buf->f_fsid = 0;
    buf->f_flag = 0;
    buf->f_namemax = 255;
    return 0;
}

int fstatvfs(int fd, struct statvfs *buf) {
    (void)fd;
    return statvfs("/", buf);
}

ssize_t getrandom(void *buf, size_t buflen, unsigned int flags) {
    static uint32_t state = 0xC001D00Du;
    unsigned char *p = buf;
    size_t i;
    (void)flags;
    if (!buf) {
        errno = EFAULT;
        return -1;
    }
    for (i = 0; i < buflen; i++) {
        state = state * 1664525u + 1013904223u;
        p[i] = (unsigned char)(state >> 24);
    }
    return (ssize_t)buflen;
}

int utime(const char *path, const struct utimbuf *times) {
    struct timespec ts[2];
    if (times) {
        ts[0].tv_sec = times->actime;
        ts[0].tv_nsec = 0;
        ts[1].tv_sec = times->modtime;
        ts[1].tv_nsec = 0;
        return utimensat(AT_FDCWD, path, ts, 0);
    }
    return utimensat(AT_FDCWD, path, NULL, 0);
}

int h_errno = 0;

struct servent *getservbyname(const char *name, const char *proto) {
    (void)name;
    (void)proto;
    h_errno = HOST_NOT_FOUND;
    return NULL;
}

struct servent *getservbyport(int port, const char *proto) {
    (void)port;
    (void)proto;
    h_errno = HOST_NOT_FOUND;
    return NULL;
}

const char *hstrerror(int err) {
    (void)err;
    return "host lookup failed";
}

int dup(int oldfd) {
    return fcntl(oldfd, F_DUPFD, 0);
}

int fsync(int fd) {
    (void)fd;
    return 0;
}

unsigned alarm(unsigned seconds) {
    (void)seconds;
    return 0;
}

int setitimer(int which, const struct itimerval *new_value,
              struct itimerval *old_value) {
    (void)which;
    (void)new_value;
    if (old_value) {
        old_value->it_interval.tv_sec = 0;
        old_value->it_interval.tv_usec = 0;
        old_value->it_value.tv_sec = 0;
        old_value->it_value.tv_usec = 0;
    }
    return 0;
}

pid_t getppid(void) {
    return 1;
}

int execl(const char *path, const char *arg, ...) {
    char *argv[64];
    int n = 0;
    va_list ap;
    argv[n++] = (char *)arg;
    va_start(ap, arg);
    while (n < 63) {
        char *a = va_arg(ap, char *);
        argv[n++] = a;
        if (!a)
            break;
    }
    va_end(ap);
    argv[63] = NULL;
    return execve(path, argv, environ);
}
