/* tcp-listen-smoke: boot-CI guest test for netd listen/accept.
 *
 * Plan 9 flow over /net/tcp: bind() stores the port, listen() announces it
 * (ctl "announce <port>"), accept() writes ctl "accept" and polls the
 * listener status for "accepted <N>". The boot test's host side connects
 * from the runner through QEMU slirp hostfwd and sends "ping"; the smoke
 * answers "pong", then sends BULK_LINES numbered lines (far more than netd
 * buffers: the writes wait for room) and closes. The host checks every byte
 * and connects again to report "good" (or "bad <bytes>").
 */
#include <arpa/inet.h>
#include <errno.h>
#include <stdio.h>
#include <netinet/in.h>
#include <string.h>
#include <sys/socket.h>
#include <sys/select.h>
#include <unistd.h>

#define LISTEN_PORT 2323
#define BULK_LINES 40000 /* "%06d\n": 280000 bytes */

/* A blocking accept on `l`, then the first bytes the peer sends. */
static int accept_read(int l, char *buf, size_t cap, ssize_t *n) {
    struct sockaddr_in peer;
    socklen_t plen = sizeof peer;
    int c = accept(l, (struct sockaddr *)&peer, &plen);
    if (c < 0) {
        return -1;
    }
    /* Data may lag the accept: netfs data reads return 0 when empty.
     * select(0,...) sleeps without spinning (libgloss pollselect). */
    for (int tries = 0; tries < 100; tries++) {
        *n = read(c, buf, cap);
        if (*n > 0) {
            break;
        }
        struct timeval tv = {0, 100 * 1000};
        select(0, NULL, NULL, NULL, &tv);
    }
    return c;
}

/* The numbered lines, a thousand per write. */
static int send_bulk(int c) {
    static char buf[7 * 1000 + 1];
    for (int line = 0; line < BULK_LINES;) {
        size_t n = 0;
        while (n < sizeof buf - 1 && line < BULK_LINES) {
            snprintf(buf + n, 8, "%06d\n", line++);
            n += 7;
        }
        if (write(c, buf, n) != (ssize_t)n) {
            return -1;
        }
    }
    return 0;
}

int main(void) {
    int l = socket(AF_INET, SOCK_STREAM, 0);
    if (l < 0) {
        printf("[ FAIL ] listen socket (%d)\n", errno);
        return 1;
    }
    struct sockaddr_in sa;
    memset(&sa, 0, sizeof sa);
    sa.sin_family = AF_INET;
    sa.sin_port = htons(LISTEN_PORT);
    sa.sin_addr.s_addr = INADDR_ANY;
    if (bind(l, (struct sockaddr *)&sa, sizeof sa) < 0) {
        printf("[ FAIL ] listen bind (%d)\n", errno);
        return 1;
    }
    if (listen(l, 1) < 0) {
        printf("[ FAIL ] listen announce (%d)\n", errno);
        return 1;
    }
    if (write(2, "[ INFO ] listening on 2323\n", 27) < 0) {
        /* stderr always writable; ignore */
    }
    char buf[64];
    ssize_t n = 0;
    int c = accept_read(l, buf, sizeof buf, &n);
    if (c < 0) {
        printf("[ FAIL ] listen accept (%d)\n", errno);
        return 1;
    }
    if (n <= 0 || memcmp(buf, "ping", 4) != 0) {
        printf("[ FAIL ] listen read (n=%d errno=%d)\n", (int)n, errno);
        return 1;
    }
    if (write(c, "pong\n", 5) != 5 || send_bulk(c) < 0) {
        printf("[ FAIL ] listen write (errno %d)\n", errno);
        return 1;
    }
    /* What netd still queues goes out before its FIN (a graceful close). */
    close(c);
    /* The host's verdict on the bulk bytes, on a second connection. */
    n = 0;
    c = accept_read(l, buf, sizeof buf - 1, &n);
    if (c < 0 || n <= 0 || memcmp(buf, "good", 4) != 0) {
        buf[n > 0 ? n : 0] = '\0';
        printf("[ FAIL ] listen bulk: host says \"%s\"\n", buf);
        return 1;
    }
    close(c);
    close(l);
    printf("[ OK ] listen\n");
    return 0;
}
