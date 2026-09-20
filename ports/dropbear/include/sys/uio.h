/* Minimal <sys/uio.h> for myos (newlib lacks it; dropbear's includes.h and
 * atomicio.c need struct iovec). readv/writev resolve via libgloss if
 * present; otherwise provide safe fallbacks built on read/write. */
#ifndef DROPBEAR_MYOS_SYS_UIO_H
#define DROPBEAR_MYOS_SYS_UIO_H

#include <sys/types.h>
#include <stddef.h>

struct iovec {
	void *iov_base;
	size_t iov_len;
};

ssize_t readv(int fd, const struct iovec *iov, int iovcnt);
ssize_t writev(int fd, const struct iovec *iov, int iovcnt);

#endif /* DROPBEAR_MYOS_SYS_UIO_H */
