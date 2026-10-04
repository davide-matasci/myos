/* Runtime stubs for Git on myos: what libgloss/myos has no honest
 * implementation for. */
#include <errno.h>
#include <fcntl.h>
#include <sys/stat.h>
#include <sys/statvfs.h>
#include <sys/types.h>
#include <sys/utime.h>
#include <unistd.h>

#ifndef AT_FDCWD
#define AT_FDCWD (-100)
#endif

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

/* No filesystem statistics on myos: a plausible, roomy volume. Git asks
 * only to decide how many pack files to keep open. */
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

/* No timer signals on myos (docs/signals.md): git's progress meter never
 * ticks, the command still runs. */
unsigned alarm(unsigned seconds) {
    (void)seconds;
    return 0;
}

/* utime over libgloss's utimensat, which stores nothing (no timestamp
 * syscall yet): git's index touches succeed and change no mtime. */
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
