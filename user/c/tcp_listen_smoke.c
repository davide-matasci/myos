/* tcp-listen-smoke: boot-CI guest test for netd listen/accept.
 *
 * Plan 9 flow over /net/tcp: bind() stores the address and port, listen()
 * announces them (ctl "announce 0.0.0.0!2323"), accept() writes ctl
 * "accept" and polls the listener status for "accepted <N>". The boot
 * test's host side connects from the runner through QEMU slirp hostfwd and
 * sends "ping"; the smoke answers "pong", then sends BULK_LINES numbered
 * lines (far more than netd buffers: the writes wait for room) and closes.
 * The host checks every byte and connects again to report "good" (or "bad
 * <bytes>"). Then it connects MUTE_PROBES times without a word and closes,
 * its FIN a varying moment after the handshake: accept must return every
 * one, ended by its peer or not (netd dropped those its user had not taken
 * yet, and reset those past two waiting: a smoke behind lost the "done"
 * and waited forever, issue #630), and each must end in a
 * hangup within a few seconds (netd used to keep a connection it had not
 * seen established, the FIN with the handshake's last ACK, open for its
 * holder forever: an nginx on a VPS waited on dozens of those until netd
 * had no conversation left and reset every new connection). A last one
 * says "done". Before any of it, a second listener on the port (on any
 * address, then on 127.0.0.1) fails with EADDRINUSE, as on Linux: a second
 * nginx used to start beside the first.
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
#define MUTE_PROBES 40

/* A second listener on LISTEN_PORT at `addr`: its listen must fail with
 * EADDRINUSE, SO_REUSEADDR or not. A failure says which call failed with
 * which errno: the socket's own (a conversation netfs could not make) is
 * not the check's verdict, and setsockopt on -1 used to replace it with
 * ENOTSOCK (issue #574: it failed so once, cause unknown). */
static int taken_twice(in_addr_t addr) {
    const char *where = addr ? "127.0.0.1" : "any";
    int s = socket(AF_INET, SOCK_STREAM, 0);
    int one = 1;
    struct sockaddr_in sa;
    if (s < 0) {
        printf("[ FAIL ] second listener on %s: socket failed (errno %d)\n", where, errno);
        return -1;
    }
    memset(&sa, 0, sizeof sa);
    sa.sin_family = AF_INET;
    sa.sin_port = htons(LISTEN_PORT);
    sa.sin_addr.s_addr = addr;
    (void)setsockopt(s, SOL_SOCKET, SO_REUSEADDR, &one, sizeof one);
    const char *call = "bind";
    int r = bind(s, (struct sockaddr *)&sa, sizeof sa);
    if (r == 0) {
        call = "listen";
        r = listen(s, 1);
    }
    int err = errno;
    close(s);
    if (r == 0) {
        printf("[ FAIL ] second listener on %s: listening\n", where);
        return -1;
    }
    if (err != EADDRINUSE) {
        printf("[ FAIL ] second listener on %s: %s failed (errno %d)\n", where, call, err);
        return -1;
    }
    return 0;
}

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

/* The mute connections, up to the one that says "done": each must end
 * (readable, then the read says hangup or data) within 3 s of its accept. */
static int mute_probes(int l) {
    for (int k = 0; k <= MUTE_PROBES; k++) {
        struct sockaddr_in peer;
        socklen_t plen = sizeof peer;
        int c = accept(l, (struct sockaddr *)&peer, &plen);
        if (c < 0) {
            printf("[ FAIL ] mute accept (%d)\n", errno);
            return -1;
        }
        fd_set r;
        FD_ZERO(&r);
        FD_SET(c, &r);
        struct timeval tv = {3, 0};
        int rv = select(c + 1, &r, NULL, NULL, &tv);
        if (rv <= 0) {
            printf("[ FAIL ] mute %d: not hung up 3 s after accept (select %d errno %d)\n", k, rv, errno);
            return -1;
        }
        char buf[8];
        ssize_t n = read(c, buf, sizeof buf - 1);
        close(c);
        if (n >= 4 && memcmp(buf, "done", 4) == 0) {
            return k;
        }
        if (n > 0) {
            buf[n] = '\0';
            printf("[ FAIL ] mute %d: said \"%s\"\n", k, buf);
            return -1;
        }
    }
    printf("[ FAIL ] mute: no \"done\" after %d connections\n", MUTE_PROBES + 1);
    return -1;
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
    if (taken_twice(INADDR_ANY) < 0 || taken_twice(htonl(INADDR_LOOPBACK)) < 0) {
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
    int mutes = mute_probes(l);
    if (mutes < 0) {
        return 1;
    }
    if (mutes != MUTE_PROBES) {
        printf("[ FAIL ] listen: %d of %d mute connections accepted\n", mutes, MUTE_PROBES);
        return 1;
    }
    close(l);
    printf("[ OK ] listen (%d mute connections)\n", mutes);
    return 0;
}
