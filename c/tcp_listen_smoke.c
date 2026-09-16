/* tcp-listen-smoke: boot-CI guest test for netd listen/accept.
 *
 * Plan 9 flow over /net/tcp: bind() stores the port, listen() announces it
 * (ctl "announce <port>"), accept() writes ctl "accept" and polls the
 * listener status for "accepted <N>". The accepted connection echoes one
 * line back to the peer. The CI harness (wait_ci) connects from the runner
 * through QEMU slirp hostfwd, sends "ping", and expects "pong".
 */
#include <arpa/inet.h>
#include <errno.h>
#include <stdio.h>
#include <netinet/in.h>
#include <string.h>
#include <sys/socket.h>
#include <unistd.h>

#define LISTEN_PORT 2323

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
    struct sockaddr_in peer;
    socklen_t plen = sizeof peer;
    int c = accept(l, (struct sockaddr *)&peer, &plen);
    if (c < 0) {
        printf("[ FAIL ] listen accept (%d)\n", errno);
        return 1;
    }
    char buf[64];
    ssize_t n = read(c, buf, sizeof buf);
    if (n <= 0 || memcmp(buf, "ping", 4) != 0) {
        printf("[ FAIL ] listen read (n=%zd)\n", n);
        return 1;
    }
    if (write(c, "pong\n", 5) != 5) {
        printf("[ FAIL ] listen write\n");
        return 1;
    }
    close(c);
    close(l);
    printf("[ OK ] listen\n");
    return 0;
}