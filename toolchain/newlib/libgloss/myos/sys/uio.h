#ifndef _MYOS_SYS_UIO_H_
#define _MYOS_SYS_UIO_H_

/* Scatter/gather I/O (uio.c): readv and writev over plain read and write,
 * no syscall of their own. */

#include <sys/types.h>

struct iovec {
    void *iov_base;
    size_t iov_len;
};

#ifndef IOV_MAX
#define IOV_MAX 1024
#endif
#define UIO_MAXIOV IOV_MAX

ssize_t readv(int fd, const struct iovec *iov, int iovcnt);
ssize_t writev(int fd, const struct iovec *iov, int iovcnt);

#endif
