/* urandom-smoke: boot-CI guest test for /dev/urandom.
 *
 * Reads two 16-byte blocks from /dev/urandom, verifies the kernel CSPRNG
 * (ChaCha20 DRBG, kernel/src/rng.rs) returns non-zero, distinct data, and
 * that successive reads differ. Prints hex + [ OK ] urandom, exit 0/1.
 */
#include <errno.h>
#include <fcntl.h>
#include <stdio.h>
#include <string.h>
#include <unistd.h>

static int nonzero(const unsigned char *b, size_t n) {
    for (size_t i = 0; i < n; i++) {
        if (b[i] != 0) return 1;
    }
    return 0;
}

static void hexdump(const unsigned char *b, size_t n, char *out) {
    static const char d[] = "0123456789abcdef";
    for (size_t i = 0; i < n; i++) {
        *out++ = d[b[i] >> 4];
        *out++ = d[b[i] & 0xf];
    }
    *out = 0;
}

int main(void) {
    unsigned char a[16], b[16];
    int fd = open("/dev/urandom", 0 /* O_RDONLY */);
    if (fd < 0) {
        printf("[ FAIL ] urandom open (%d)\n", errno);
        return 1;
    }
    if (read(fd, a, sizeof(a)) != (ssize_t)sizeof(a)) {
        printf("[ FAIL ] urandom read a (%d)\n", errno);
        return 1;
    }
    if (read(fd, b, sizeof(b)) != (ssize_t)sizeof(b)) {
        printf("[ FAIL ] urandom read b (%d)\n", errno);
        return 1;
    }
    close(fd);
    if (!nonzero(a, sizeof(a)) || !nonzero(b, sizeof(b))) {
        printf("[ FAIL ] urandom all-zero\n");
        return 1;
    }
    if (memcmp(a, b, sizeof(a)) == 0) {
        printf("[ FAIL ] urandom blocks identical\n");
        return 1;
    }
    char ha[33], hb[33];
    hexdump(a, sizeof(a), ha);
    hexdump(b, sizeof(b), hb);
    printf("[ INFO ] urandom %s %s\n", ha, hb);
    printf("[ OK ] urandom\n");
    return 0;
}