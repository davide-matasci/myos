/* loopback-smoke: boot-CI guest test for netd's loopback interface, UDP
 * and the interface list.
 *
 * getifaddrs lists lo (127.0.0.1, IFF_LOOPBACK) and net0 with an address,
 * if_nametoindex/if_indextoname agree with it. UDP over 127.0.0.1: bind to
 * port 0 picks one (getsockname), datagrams both ways with their source
 * (recvfrom), a second bind of a taken port fails, connect and unconnect
 * (AF_UNSPEC), a datagram to a port nobody bound refuses the next send
 * (ECONNREFUSED), the host's own address is local too. TCP: a listener
 * accepts a connection to 127.0.0.1 and both ends talk (a MSG_PEEK first,
 * which leaves the bytes for the read); one bound to
 * 127.0.0.1 has that address (getsockname), accepts there and refuses a
 * connection to the host's address, and an address not the host's is not
 * bindable (EADDRNOTAVAIL). A connection whose peer hung up keeps its
 * conversation: the next connection accepted, on another listener, is
 * not seen on the old socket. An unconnected UDP
 * socket passed by exec: the new program reads a bare datagram with
 * read(), then uses the fd as the socket it is (getsockname, sendto,
 * recvfrom). Prints [ OK ] loopback.
 *
 * `loopback_smoke burst`: BURST short TCP connections back to back, the
 * client closing first, then one more that talks: each leaves its client
 * socket in TIME-WAIT, buffers and all, and netd used to keep them all
 * until its heap ran out (it died of it, every socket call after it
 * waiting). Prints [ OK ] loopback burst.
 */
#include <arpa/inet.h>
#include <errno.h>
#include <ifaddrs.h>
#include <net/if.h>
#include <netinet/in.h>
#include <stdio.h>
#include <string.h>
#include <stdlib.h>
#include <sys/socket.h>
#include <sys/wait.h>
#include <unistd.h>

#define TCP_PORT 7701
#define CLOSED_PORT 7702
#define BOUND_PORT 7703
#define HUNG_PORT 7704
#define NEXT_PORT 7705
#define BURST_PORT 7706
/* More than netd's heap held in TIME-WAIT (about 165). */
#define BURST 200

static int fail(const char *what) {
    printf("[ FAIL ] loopback %s (errno %d)\n", what, errno);
    return 1;
}

static struct sockaddr_in addr_of(in_addr_t a, unsigned short port) {
    struct sockaddr_in sin;
    memset(&sin, 0, sizeof sin);
    sin.sin_family = AF_INET;
    sin.sin_addr.s_addr = a;
    sin.sin_port = htons(port);
    return sin;
}

static int local_of(int fd, struct sockaddr_in *sin) {
    socklen_t len = sizeof *sin;
    return getsockname(fd, (struct sockaddr *)sin, &len);
}

/* The next datagram on `fd` (blocking) is `want`, from `from`. */
static int recv_is(int fd, const char *want, const struct sockaddr_in *from) {
    char buf[32];
    struct sockaddr_in src;
    socklen_t len = sizeof src;
    ssize_t n = recvfrom(fd, buf, sizeof buf, 0, (struct sockaddr *)&src, &len);
    return n == (ssize_t)strlen(want) && memcmp(buf, want, (size_t)n) == 0
        && src.sin_addr.s_addr == from->sin_addr.s_addr && src.sin_port == from->sin_port;
}

static int interfaces(in_addr_t *own) {
    struct ifaddrs *ifa;
    struct ifaddrs *i;
    char name[IF_NAMESIZE];
    int lo = 0;
    if (getifaddrs(&ifa) < 0) {
        return fail("getifaddrs");
    }
    *own = 0;
    for (i = ifa; i != NULL; i = i->ifa_next) {
        const struct sockaddr_in *a = (const struct sockaddr_in *)i->ifa_addr;
        if (a == NULL || a->sin_family != AF_INET) {
            continue;
        }
        if (strcmp(i->ifa_name, "lo") == 0 && a->sin_addr.s_addr == htonl(INADDR_LOOPBACK)
            && (i->ifa_flags & IFF_LOOPBACK)) {
            lo = 1;
        }
        if (strcmp(i->ifa_name, "net0") == 0 && (i->ifa_flags & IFF_UP)) {
            *own = a->sin_addr.s_addr;
        }
    }
    freeifaddrs(ifa);
    if (!lo || *own == 0) {
        return fail("getifaddrs: no lo or net0 address");
    }
    if (if_nametoindex("lo") != 1 || if_indextoname(2, name) == NULL
        || strcmp(name, "net0") != 0 || if_nametoindex("eth9") != 0) {
        return fail("if_nametoindex");
    }
    return 0;
}

static int udp(in_addr_t own) {
    struct sockaddr_in loopback = addr_of(htonl(INADDR_LOOPBACK), 0);
    struct sockaddr_in any = addr_of(htonl(INADDR_ANY), 0);
    struct sockaddr_in a_addr;
    struct sockaddr_in b_addr;
    struct sockaddr_in to;
    struct sockaddr_in unspec;
    socklen_t len;
    int a = socket(AF_INET, SOCK_DGRAM, 0);
    int b = socket(AF_INET, SOCK_DGRAM, 0);
    int c = socket(AF_INET, SOCK_DGRAM, 0);
    int d = socket(AF_INET, SOCK_DGRAM, 0);
    char x = 'x';
    if (a < 0 || b < 0 || c < 0 || d < 0) {
        return fail("udp socket");
    }
    if (bind(a, (struct sockaddr *)&loopback, sizeof loopback) < 0 || local_of(a, &a_addr) < 0
        || a_addr.sin_addr.s_addr != htonl(INADDR_LOOPBACK) || a_addr.sin_port == 0) {
        return fail("udp bind 127.0.0.1:0");
    }
    /* b's first send binds it a port. */
    if (sendto(b, "ping", 4, 0, (struct sockaddr *)&a_addr, sizeof a_addr) != 4
        || local_of(b, &b_addr) < 0 || b_addr.sin_port == 0) {
        return fail("udp sendto");
    }
    b_addr.sin_addr.s_addr = htonl(INADDR_LOOPBACK);
    if (!recv_is(a, "ping", &b_addr)) {
        return fail("udp recvfrom ping");
    }
    if (sendto(a, "pong", 4, 0, (struct sockaddr *)&b_addr, sizeof b_addr) != 4
        || !recv_is(b, "pong", &a_addr)) {
        return fail("udp pong");
    }
    /* Taken unless both ask for SO_REUSEADDR. */
    errno = 0;
    if (bind(c, (struct sockaddr *)&a_addr, sizeof a_addr) == 0 || errno != EADDRINUSE) {
        return fail("udp bind of a taken port");
    }
    /* Connected: send() to the peer; unconnected again: no peer. */
    if (connect(b, (struct sockaddr *)&a_addr, sizeof a_addr) < 0 || send(b, "conn", 4, 0) != 4
        || !recv_is(a, "conn", &b_addr)) {
        return fail("udp connect");
    }
    memset(&unspec, 0, sizeof unspec);
    unspec.sin_family = AF_UNSPEC;
    len = sizeof to;
    if (connect(b, (struct sockaddr *)&unspec, sizeof unspec) < 0
        || getpeername(b, (struct sockaddr *)&to, &len) == 0 || errno != ENOTCONN) {
        return fail("udp unconnect");
    }
    /* Nobody on that port: ICMP port unreachable, the next send fails. */
    to = addr_of(htonl(INADDR_LOOPBACK), CLOSED_PORT);
    if (connect(c, (struct sockaddr *)&to, sizeof to) < 0 || send(c, &x, 1, 0) != 1) {
        return fail("udp send to a closed port");
    }
    usleep(200 * 1000);
    errno = 0;
    if (send(c, &x, 1, 0) >= 0 || errno != ECONNREFUSED) {
        return fail("udp: no ECONNREFUSED");
    }
    /* The host's own address is local too (b's unconnect gave up the
     * port its first send bound: this send binds another). */
    if (bind(d, (struct sockaddr *)&any, sizeof any) < 0 || local_of(d, &to) < 0) {
        return fail("udp bind 0.0.0.0:0");
    }
    to.sin_addr.s_addr = own;
    if (sendto(b, "own", 3, 0, (struct sockaddr *)&to, sizeof to) != 3
        || local_of(b, &b_addr) < 0) {
        return fail("udp sendto the own address");
    }
    b_addr.sin_addr.s_addr = own;
    if (!recv_is(d, "own", &b_addr)) {
        return fail("udp from the own address");
    }
    close(a);
    close(b);
    close(c);
    close(d);
    return 0;
}

static int tcp(void) {
    struct sockaddr_in any = addr_of(htonl(INADDR_ANY), TCP_PORT);
    struct sockaddr_in to = addr_of(htonl(INADDR_LOOPBACK), TCP_PORT);
    char buf[8];
    int l = socket(AF_INET, SOCK_STREAM, 0);
    int c = socket(AF_INET, SOCK_STREAM, 0);
    int s;
    if (l < 0 || c < 0 || bind(l, (struct sockaddr *)&any, sizeof any) < 0 || listen(l, 4) < 0) {
        return fail("tcp listen");
    }
    if (connect(c, (struct sockaddr *)&to, sizeof to) < 0) {
        return fail("tcp connect 127.0.0.1");
    }
    s = accept(l, NULL, NULL);
    if (s < 0) {
        return fail("tcp accept");
    }
    if (write(c, "hi", 2) != 2) {
        return fail("tcp write");
    }
    /* MSG_PEEK leaves the bytes for the next read. */
    if (recv(s, buf, sizeof buf, MSG_PEEK) != 2 || memcmp(buf, "hi", 2) != 0) {
        return fail("tcp peek");
    }
    if (read(s, buf, sizeof buf) != 2 || memcmp(buf, "hi", 2) != 0
        || write(s, "yo", 2) != 2 || read(c, buf, sizeof buf) != 2 || memcmp(buf, "yo", 2) != 0) {
        return fail("tcp exchange");
    }
    close(c);
    close(s);
    close(l);
    return 0;
}

/* A connection whose peer hung up keeps its conversation until this end
 * closes it: the next connection accepted, on another listener, must not
 * show up on the old socket (netd used to free the index at the hangup and
 * hand it to the next connection while this end still read and wrote it). */
static int tcp_hungup(void) {
    struct sockaddr_in any1 = addr_of(htonl(INADDR_ANY), HUNG_PORT);
    struct sockaddr_in any2 = addr_of(htonl(INADDR_ANY), NEXT_PORT);
    struct sockaddr_in to1 = addr_of(htonl(INADDR_LOOPBACK), HUNG_PORT);
    struct sockaddr_in to2 = addr_of(htonl(INADDR_LOOPBACK), NEXT_PORT);
    struct timeval tv = {1, 0};
    char buf[8];
    int l1 = socket(AF_INET, SOCK_STREAM, 0);
    int l2 = socket(AF_INET, SOCK_STREAM, 0);
    int c1 = socket(AF_INET, SOCK_STREAM, 0);
    int c2 = socket(AF_INET, SOCK_STREAM, 0);
    int s1;
    int s2;
    if (l1 < 0 || l2 < 0 || c1 < 0 || c2 < 0
        || bind(l1, (struct sockaddr *)&any1, sizeof any1) < 0 || listen(l1, 4) < 0
        || bind(l2, (struct sockaddr *)&any2, sizeof any2) < 0 || listen(l2, 4) < 0) {
        return fail("tcp hungup listen");
    }
    if (connect(c1, (struct sockaddr *)&to1, sizeof to1) < 0 || (s1 = accept(l1, NULL, NULL)) < 0) {
        return fail("tcp hungup accept");
    }
    /* The peer hangs up; netd closes the socket within a few ticks. */
    close(c1);
    select(0, NULL, NULL, NULL, &tv);
    if (connect(c2, (struct sockaddr *)&to2, sizeof to2) < 0 || (s2 = accept(l2, NULL, NULL)) < 0) {
        return fail("tcp hungup next accept");
    }
    if (write(c2, "new", 3) != 3 || write(s2, "ok", 2) != 2) {
        return fail("tcp hungup next write");
    }
    /* The old socket: end of file, nothing of the new connection; what is
     * written to it goes nowhere (netd dropped its socket at the hangup),
     * not to the new connection's peer. */
    if (read(s1, buf, sizeof buf) > 0) {
        return fail("tcp hungup read");
    }
    (void)write(s1, "old", 3);
    if (read(c2, buf, sizeof buf) != 2 || memcmp(buf, "ok", 2) != 0) {
        return fail("tcp hungup next read");
    }
    if (read(s2, buf, sizeof buf) != 3 || memcmp(buf, "new", 3) != 0) {
        return fail("tcp hungup next data");
    }
    close(s1);
    close(c2);
    close(s2);
    close(l1);
    close(l2);
    return 0;
}

/* A listener on 127.0.0.1 only: not on `own`, the host's address. */
static int tcp_bound(in_addr_t own) {
    struct sockaddr_in lo = addr_of(htonl(INADDR_LOOPBACK), BOUND_PORT);
    struct sockaddr_in host = addr_of(own, BOUND_PORT);
    struct sockaddr_in other = addr_of(inet_addr("192.0.2.1"), BOUND_PORT);
    struct sockaddr_in got;
    char buf[8];
    int l = socket(AF_INET, SOCK_STREAM, 0);
    int c = socket(AF_INET, SOCK_STREAM, 0);
    int d = socket(AF_INET, SOCK_STREAM, 0);
    int s;
    if (l < 0 || c < 0 || d < 0) {
        return fail("tcp bound socket");
    }
    if (bind(d, (struct sockaddr *)&other, sizeof other) == 0 || errno != EADDRNOTAVAIL) {
        return fail("tcp bind of an address not the host's");
    }
    if (bind(l, (struct sockaddr *)&lo, sizeof lo) < 0 || listen(l, 4) < 0) {
        return fail("tcp listen on 127.0.0.1");
    }
    if (local_of(l, &got) < 0 || got.sin_addr.s_addr != lo.sin_addr.s_addr
        || got.sin_port != lo.sin_port) {
        return fail("tcp getsockname of the listener");
    }
    if (connect(d, (struct sockaddr *)&host, sizeof host) == 0 || errno != ECONNREFUSED) {
        return fail("tcp connect to the host's address refused");
    }
    if (connect(c, (struct sockaddr *)&lo, sizeof lo) < 0) {
        return fail("tcp connect to the bound listener");
    }
    s = accept(l, NULL, NULL);
    if (s < 0 || write(c, "hi", 2) != 2 || read(s, buf, sizeof buf) != 2) {
        return fail("tcp bound exchange");
    }
    close(c);
    close(d);
    close(s);
    close(l);
    return 0;
}

/* The exec'd side of exec_udp: `fd` is the socket, bound to `port`. */
static int exec_child(int fd, unsigned short port) {
    char buf[8];
    struct sockaddr_in local;
    struct sockaddr_in from;
    socklen_t len = sizeof from;
    /* read() knows nothing of sockets: the bare datagram. */
    if (read(fd, buf, sizeof buf) != 2 || memcmp(buf, "ab", 2) != 0) {
        return fail("exec: read of the inherited socket");
    }
    if (local_of(fd, &local) < 0 || local.sin_port != htons(port)) {
        return fail("exec: getsockname of the inherited socket");
    }
    if (recvfrom(fd, buf, sizeof buf, 0, (struct sockaddr *)&from, &len) != 2
        || memcmp(buf, "cd", 2) != 0
        || sendto(fd, "ef", 2, 0, (struct sockaddr *)&from, sizeof from) != 2) {
        return fail("exec: recvfrom/sendto on the inherited socket");
    }
    return 0;
}

static int exec_udp(void) {
    struct sockaddr_in loopback = addr_of(htonl(INADDR_LOOPBACK), 0);
    struct sockaddr_in a_addr;
    char fd_arg[8];
    char port_arg[8];
    char buf[8];
    int status;
    pid_t pid;
    int a = socket(AF_INET, SOCK_DGRAM, 0);
    int b = socket(AF_INET, SOCK_DGRAM, 0);
    if (a < 0 || b < 0 || bind(a, (struct sockaddr *)&loopback, sizeof loopback) < 0
        || local_of(a, &a_addr) < 0
        || sendto(b, "ab", 2, 0, (struct sockaddr *)&a_addr, sizeof a_addr) != 2
        || sendto(b, "cd", 2, 0, (struct sockaddr *)&a_addr, sizeof a_addr) != 2) {
        return fail("exec: setup");
    }
    usleep(100 * 1000);
    snprintf(fd_arg, sizeof fd_arg, "%d", a);
    snprintf(port_arg, sizeof port_arg, "%u", ntohs(a_addr.sin_port));
    pid = fork();
    if (pid == 0) {
        execl("/bin/etc/loopback_smoke", "loopback_smoke", "exec", fd_arg, port_arg, (char *)NULL);
        _exit(127);
    }
    if (pid < 0 || waitpid(pid, &status, 0) != pid || !WIFEXITED(status)
        || WEXITSTATUS(status) != 0) {
        return fail("exec: the exec'd program");
    }
    if (recv(b, buf, sizeof buf, 0) != 2 || memcmp(buf, "ef", 2) != 0) {
        return fail("exec: the exec'd program's answer");
    }
    close(a);
    close(b);
    return 0;
}

/* BURST connections, each closed by the client first, then one that talks. */
static int tcp_burst(void) {
    struct sockaddr_in any = addr_of(htonl(INADDR_ANY), BURST_PORT);
    struct sockaddr_in to = addr_of(htonl(INADDR_LOOPBACK), BURST_PORT);
    char buf[8];
    int l = socket(AF_INET, SOCK_STREAM, 0);
    int c, s;
    if (l < 0 || bind(l, (struct sockaddr *)&any, sizeof any) < 0 || listen(l, 4) < 0) {
        return fail("burst listen");
    }
    for (int i = 0; i <= BURST; i++) {
        c = socket(AF_INET, SOCK_STREAM, 0);
        if (c < 0 || connect(c, (struct sockaddr *)&to, sizeof to) < 0) {
            printf("[ FAIL ] loopback burst: connection %d (errno %d)\n", i, errno);
            return 1;
        }
        if ((s = accept(l, NULL, NULL)) < 0) {
            printf("[ FAIL ] loopback burst: accept %d (errno %d)\n", i, errno);
            return 1;
        }
        if (i == BURST && (write(c, "hi", 2) != 2 || read(s, buf, sizeof buf) != 2 || memcmp(buf, "hi", 2) != 0)) {
            return fail("burst: the last connection's exchange");
        }
        close(c);
        close(s);
    }
    close(l);
    return 0;
}

int main(int argc, char **argv) {
    in_addr_t own;
    if (argc == 4 && strcmp(argv[1], "exec") == 0) {
        return exec_child(atoi(argv[2]), (unsigned short)atoi(argv[3]));
    }
    if (argc == 2 && strcmp(argv[1], "burst") == 0) {
        if (tcp_burst()) {
            return 1;
        }
        printf("[ OK ] loopback burst\n");
        return 0;
    }
    if (interfaces(&own) || udp(own) || tcp() || tcp_hungup() || tcp_bound(own) || exec_udp()) {
        return 1;
    }
    printf("[ OK ] loopback\n");
    return 0;
}
