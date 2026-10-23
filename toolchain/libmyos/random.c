/* getrandom(2): both libcs lack it (newlib's getentropy and mlibc's are
 * their own, over the same device). */
#include <errno.h>
#include <fcntl.h>
#include <sys/random.h>
#include <unistd.h>

#include "libc.h"

/* getrandom(2) from /dev/urandom (the kernel's generator, docs/testing.md
 * `urandom`): never blocks, so GRND_NONBLOCK and GRND_RANDOM change nothing. */
ssize_t __myos_getrandom(void *buf, size_t buflen, unsigned int flags) {
    if ((flags & ~(GRND_NONBLOCK | GRND_RANDOM | GRND_INSECURE)) != 0) {
        errno = EINVAL;
        return -1;
    }
    int fd = open("/dev/urandom", O_RDONLY);
    if (fd < 0) {
        return -1;
    }
    size_t got = 0;
    while (got < buflen) {
        ssize_t n = read(fd, (char *)buf + got, buflen - got);
        if (n < 0) {
            if (errno == EINTR && got == 0) {
                continue;
            }
            int e = errno;
            close(fd);
            if (got > 0) {
                return (ssize_t)got;
            }
            errno = e;
            return -1;
        }
        if (n == 0) {
            break;
        }
        got += (size_t)n;
    }
    close(fd);
    return (ssize_t)got;
}

MYOS_PUBLIC(getrandom)
