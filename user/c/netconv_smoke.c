/* netconv-smoke: boot-CI guest test for netfs conversation bookkeeping.
 *
 * A TCP connect to a port of 127.0.0.1 nobody listens on is refused at
 * once (netd's loopback interface answers with a reset). Many sockets
 * closed right after
 * socket(), before netd acknowledged the clone, give every conversation
 * back: after them 16 sockets open together. Prints [ OK ] netconv.
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
    printf("[ OK ] netconv\n");
    return 0;
}
