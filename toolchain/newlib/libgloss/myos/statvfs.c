/* myos libgloss: statvfs and fstatvfs, from the kernel's statfsat
 * (kernel/src/user/at.rs): the path's filesystem, or with an empty path
 * and MYOS_AT_EMPTY_PATH the fd's. */
#include <errno.h>
#include <stdint.h>
#include <string.h>
#include <sys/statvfs.h>

#include "myos_syscalls.h"

/* The kernel's MyosStatfs. */
struct myos_statfs {
    uint64_t bsize;
    uint64_t blocks;
    uint64_t bfree;
    uint64_t bavail;
    uint64_t files;
    uint64_t ffree;
    uint64_t namemax;
    uint64_t flags;
};

#define MYOS_ST_RDONLY 1

/* The longest path the kernel takes (kernel/src/user/mod.rs MAX_PATH). */
#define MYOS_MAX_PATH 256

/* fsfilcnt_t is 32 bits in newlib: a larger count reads as its largest. */
static fsfilcnt_t files(uint64_t n) {
    return n > (fsfilcnt_t)-1 ? (fsfilcnt_t)-1 : (fsfilcnt_t)n;
}

static int statfsat(long dirfd, const char *path, long len, long flags, struct statvfs *buf, int err) {
    struct myos_statfs k;
    if (buf == NULL) {
        errno = EFAULT;
        return -1;
    }
    if ((unsigned long)myos_syscall6(MYOS_SYS_STATFSAT, dirfd, (long)(uintptr_t)path, len, flags,
                                     (long)(uintptr_t)&k, 0) == MYOS_SYSERR) {
        errno = err;
        return -1;
    }
    memset(buf, 0, sizeof(*buf));
    buf->f_bsize = k.bsize;
    buf->f_frsize = k.bsize;
    buf->f_blocks = k.blocks;
    buf->f_bfree = k.bfree;
    buf->f_bavail = k.bavail;
    buf->f_files = files(k.files);
    buf->f_ffree = files(k.ffree);
    buf->f_favail = files(k.ffree);
    buf->f_flag = k.flags & MYOS_ST_RDONLY ? ST_RDONLY : 0;
    buf->f_namemax = k.namemax;
    return 0;
}

int statvfs(const char *__restrict path, struct statvfs *__restrict buf) {
    size_t n;
    if (path == NULL) {
        errno = EFAULT;
        return -1;
    }
    n = strlen(path);
    if (n == 0) {
        errno = ENOENT;
        return -1;
    }
    if (n > MYOS_MAX_PATH) {
        errno = ENAMETOOLONG;
        return -1;
    }
    return statfsat(MYOS_AT_FDCWD, path, (long)n, 0, buf, ENOENT);
}

int fstatvfs(int fd, struct statvfs *buf) {
    return statfsat(fd, "", 0, MYOS_AT_EMPTY_PATH, buf, EBADF);
}
