/* uio-smoke: boot-CI guest test for readv/writev (libgloss uio.c).
 *
 * writev of several buffers (an empty one among them) through a pipe and a
 * readv that scatters one read; an AF_UNIX socketpair getting a header and
 * its payload from one writev; a nonblocking socket whose buffer fills
 * mid-writev returning the short count, with exactly those bytes arriving;
 * the one-by-one path past the gather size (to /dev/null); EINVAL for a bad
 * count. Prints [ OK ] uio.
 */
#include <errno.h>
#include <fcntl.h>
#include <limits.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/socket.h>
#include <sys/uio.h>
#include <unistd.h>

static int fail(const char *what) {
    printf("[ FAIL ] uio %s (errno %d)\n", what, errno);
    return 1;
}

/* Read exactly n bytes (the socket reads never wait: retry until there). */
static int read_all(int fd, char *buf, size_t n) {
    size_t got = 0;
    for (int tries = 0; got < n && tries < 100000; tries++) {
        ssize_t r = read(fd, buf + got, n - got);
        if (r < 0 && errno != EAGAIN) {
            return -1;
        }
        if (r > 0) {
            got += (size_t)r;
        }
    }
    return got == n ? 0 : -1;
}

int main(void) {
    /* A pipe: gather three buffers, scatter into two. */
    int p[2];
    if (pipe(p) < 0) {
        return fail("pipe");
    }
    struct iovec out[3] = {{"hello", 5}, {"", 0}, {" world", 6}};
    if (writev(p[1], out, 3) != 11) {
        return fail("writev to a pipe");
    }
    char a[4], b[16];
    struct iovec in[2] = {{a, sizeof a}, {b, sizeof b}};
    if (readv(p[0], in, 2) != 11 || memcmp(a, "hell", 4) != 0 || memcmp(b, "o world", 7) != 0) {
        return fail("readv from a pipe");
    }
    close(p[0]);
    close(p[1]);

    /* A socketpair: a header and its payload in one writev. */
    int sv[2];
    if (socketpair(AF_UNIX, SOCK_STREAM, 0, sv) < 0) {
        return fail("socketpair");
    }
    static char payload[3000], got[3004];
    for (size_t i = 0; i < sizeof payload; i++) {
        payload[i] = (char)(i * 7);
    }
    struct iovec req[2] = {{"HDR:", 4}, {payload, sizeof payload}};
    if (writev(sv[0], req, 2) != (ssize_t)sizeof got) {
        return fail("writev to a socket");
    }
    if (read_all(sv[1], got, sizeof got) < 0 || memcmp(got, "HDR:", 4) != 0
        || memcmp(got + 4, payload, sizeof payload) != 0) {
        return fail("socket bytes");
    }

    /* Nonblocking, more than the peer buffers: a short count, those bytes. */
    if (fcntl(sv[0], F_SETFL, fcntl(sv[0], F_GETFL) | O_NONBLOCK) < 0) {
        return fail("O_NONBLOCK");
    }
    static char big[3][4096], back[3 * 4096];
    for (int i = 0; i < 3; i++) {
        memset(big[i], 'a' + i, sizeof big[i]);
    }
    struct iovec fill[3] = {{big[0], 4096}, {big[1], 4096}, {big[2], 4096}};
    ssize_t n = writev(sv[0], fill, 3);
    if (n <= 0 || n >= 3 * 4096) {
        printf("[ FAIL ] uio short writev: %ld of %d\n", (long)n, 3 * 4096);
        return 1;
    }
    if (read_all(sv[1], back, (size_t)n) < 0) {
        return fail("reading the short writev");
    }
    for (ssize_t i = 0; i < n; i++) {
        if (back[i] != 'a' + i / 4096) {
            printf("[ FAIL ] uio byte %ld of the short writev\n", (long)i);
            return 1;
        }
    }
    close(sv[0]);
    close(sv[1]);

    /* Past the gather buffer: written one by one. */
    int null = open("/dev/null", O_WRONLY);
    char *huge = calloc(1, 40000);
    struct iovec two[2] = {{huge, 40000}, {huge, 40000}};
    if (null < 0 || !huge || writev(null, two, 2) != 80000) {
        return fail("writev past the gather size");
    }

    errno = 0;
    if (writev(null, two, -1) != -1 || errno != EINVAL) {
        return fail("negative count not EINVAL");
    }
    errno = 0;
    if (readv(null, in, IOV_MAX + 1) != -1 || errno != EINVAL) {
        return fail("count past IOV_MAX not EINVAL");
    }
    printf("[ OK ] uio\n");
    return 0;
}
