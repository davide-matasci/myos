/* getrandom(2): both libcs lack it (newlib's getentropy and mlibc's are
 * their own, over the same device). And the BSDs' arc4random family, which
 * newlib has of its own and mlibc lacks (LibreSSL takes it from the libc). */
#include <errno.h>
#include <fcntl.h>
#include <stdint.h>
#include <stdlib.h>
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

/* arc4random_buf(3): `n` bytes from the kernel's generator, every call
 * (no generator of its own in the process: nothing to reseed after a
 * fork). It cannot fail: a generator that does not answer aborts. */
void __myos_arc4random_buf(void *buf, size_t n) {
    while (n > 0) {
        ssize_t got = __myos_getrandom(buf, n, 0);
        if (got <= 0) {
            abort();
        }
        buf = (char *)buf + got;
        n -= (size_t)got;
    }
}

uint32_t __myos_arc4random(void) {
    uint32_t r;
    __myos_arc4random_buf(&r, sizeof(r));
    return r;
}

/* A number below `upper` without modulo bias: the values under
 * 2^32 mod upper are drawn again. */
uint32_t __myos_arc4random_uniform(uint32_t upper) {
    if (upper < 2) {
        return 0;
    }
    uint32_t min = -upper % upper;
    for (;;) {
        uint32_t r = __myos_arc4random();
        if (r >= min) {
            return r % upper;
        }
    }
}

MYOS_MLIBC_PUBLIC(arc4random_buf)
MYOS_MLIBC_PUBLIC(arc4random)
MYOS_MLIBC_PUBLIC(arc4random_uniform)
