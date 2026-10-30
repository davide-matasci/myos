/* netconv-smoke: boot-CI guest test for netfs conversation bookkeeping.
 *
 * A TCP connect to a port of 127.0.0.1 nobody listens on is refused at
 * once (netd's loopback interface answers with a reset). Many sockets
 * closed right after
 * socket(), before netd acknowledged the clone, give every conversation
 * back: after them 16 sockets open together. A burst of sends, more than
 * netfs's request ring to netd holds, all go through (each waits for
 * room in the ring; a full one refused the request, and `socket()` with
 * it, issue #574), and a socket() right after them succeeds. Prints
 * [ OK ] netconv.
 */
#include <arpa/inet.h>
#include <errno.h>
#include <netinet/in.h>
#include <stdio.h>
#include <string.h>
#include <sys/socket.h>
#include <unistd.h>

#define CHURN 200
#define HELD 16
/* Datagrams sent back to back: well over the ring's 128 requests, since
 * netd drains it meanwhile. */
#define BURST 600

static int fail(const char *what) {
    printf("[ FAIL ] netconv %s (errno %d)\n", what, errno);
    return 1;
}

/* socket() for TCP; closing conversations wait for netd's ack, so a
 * transient failure is retried for a few seconds. */
static int tcp_socket(void) {
    for (int tries = 0; tries < 100; tries++) {
        int fd = socket(AF_INET, SOCK_STREAM, 0);
        if (fd >= 0) {
            return fd;
        }
        usleep(100 * 1000);
    }
    return -1;
}

/* BURST one-byte datagrams to a loopback port nobody listens on, as fast
 * as the syscalls go: netd takes them from netfs's ring slower than that,
 * so the ring fills, and every send still succeeds. Then a socket(). */
static int burst(void) {
    struct sockaddr_in sin;
    int fd = socket(AF_INET, SOCK_DGRAM, 0);
    int s;
    if (fd < 0) {
        return fail("udp socket");
    }
    memset(&sin, 0, sizeof sin);
    sin.sin_family = AF_INET;
    sin.sin_port = htons(6001);
    sin.sin_addr.s_addr = htonl(INADDR_LOOPBACK);
    for (int i = 0; i < BURST; i++) {
        if (sendto(fd, "x", 1, 0, (struct sockaddr *)&sin, sizeof sin) != 1) {
            printf("[ FAIL ] netconv send %d of the burst (errno %d)\n", i, errno);
            return 1;
        }
    }
    s = socket(AF_INET, SOCK_STREAM, 0);
    if (s < 0) {
        return fail("socket after the burst");
    }
    close(s);
    close(fd);
    return 0;
}

int main(void) {
    struct sockaddr_in sin;
    int fds[HELD];
    int fd;

    fd = tcp_socket();
    if (fd < 0) {
        return fail("socket");
    }
    memset(&sin, 0, sizeof sin);
    sin.sin_family = AF_INET;
    sin.sin_port = htons(6000);
    sin.sin_addr.s_addr = htonl(INADDR_LOOPBACK);
    errno = 0;
    if (connect(fd, (struct sockaddr *)&sin, sizeof sin) == 0 || errno != ECONNREFUSED) {
        return fail("loopback connect not refused");
    }
    close(fd);

    for (int i = 0; i < CHURN; i++) {
        fd = tcp_socket();
        if (fd < 0) {
            printf("[ FAIL ] netconv socket %d of the churn (errno %d)\n", i, errno);
            return 1;
        }
        close(fd);
    }
    for (int i = 0; i < HELD; i++) {
        fds[i] = tcp_socket();
        if (fds[i] < 0) {
            printf("[ FAIL ] netconv held socket %d (errno %d)\n", i, errno);
            return 1;
        }
    }
    for (int i = 0; i < HELD; i++) {
        close(fds[i]);
    }
    if (burst() != 0) {
        return 1;
    }
    printf("[ OK ] netconv\n");
    return 0;
}
