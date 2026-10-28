/* myos: WebKit (built as for Linux) reserves a file's blocks with
 * fallocate, which mlibc has for Linux alone. Not here: EOPNOTSUPP, which
 * WebKit takes as a filesystem without it (the file grows on write). */
#ifndef MYOS_COMPAT_FCNTL_H
#define MYOS_COMPAT_FCNTL_H

#include_next <fcntl.h>

#include <errno.h>

static inline int fallocate(int fd, int mode, off_t offset, off_t len)
{
    (void)fd;
    (void)mode;
    (void)offset;
    (void)len;
    errno = EOPNOTSUPP;
    return -1;
}

#endif
