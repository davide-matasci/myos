/* ipv6-smoke: boot-CI guest test for IPv6 (docs/net.md).
 *
 * getifaddrs lists lo's ::1 and net0's link-local address (its scope the
 * interface's index) and, once the router advertised, its SLAAC address,
 * which /net/ndb's ip6= line names. inet_pton/inet_ntop agree on the text
 * forms, getaddrinfo finds ::1 for localhost and a literal's own family,
 * getnameinfo names an IPv6 address. TCP over ::1: a listener accepts and
 * both ends talk, each naming the other ::1. An AF_INET6 listener on ::
 * accepts an IPv4 client, named ::ffff:127.0.0.1; with IPV6_V6ONLY it
 * shares its port with an AF_INET listener, each taking its own family's
 * connections, and without it the AF_INET one cannot have the port. UDP
 * over ::1: datagrams both ways with their source, an AF_INET6 socket on
 * :: takes an IPv4 datagram (::ffff:127.0.0.1), and a datagram to a port
 * nobody bound refuses the next send (an ICMPv6 port unreachable). An
 * ICMPv6 echo over /net/icmp comes back from ::1 and, through net0, from
 * the router /net/ndb names. Prints [ OK ] ipv6.
 *
 * `ipv6_smoke dns` (the network): getaddrinfo(AF_INET6) finds a name's
 * AAAA record and AF_UNSPEC both families, IPv4 first (newlib's), when the
 * resolver has the record.
 * Prints [ OK ] ipv6 dns.
 *
 * `ipv6_smoke dnsretry` (newlib's resolver): against a DNS server of the
 * test's own on the loopback interface that drops the first AAAA query,
 * an AF_UNSPEC lookup still gives both families, the AAAA query sent
 * again once the A answer came (CI #2369: a lost AAAA datagram left the
 * lookup with IPv4 alone). Prints [ OK ] ipv6 dnsretry.
 */
#include <arpa/inet.h>
#include <errno.h>
#include <fcntl.h>
#include <ifaddrs.h>
#include <net/if.h>
#include <netdb.h>
#include <netinet/in.h>
#include <poll.h>
#include <signal.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/socket.h>
#include <sys/wait.h>
#include <time.h>
#include <unistd.h>

#define TCP_PORT 7711
#define DUAL_PORT 7712
#define SPLIT_PORT 7713
#define CLOSED_PORT 7714
/* A name with an AAAA record. */
#define DNS_NAME "example.com"

static int fail(const char *what) {
    printf("[ FAIL ] ipv6 %s (errno %d)\n", what, errno);
    return 1;
}

static struct sockaddr_in6 in6(const char *addr, int port) {
    struct sockaddr_in6 a;
    memset(&a, 0, sizeof a);
    a.sin6_family = AF_INET6;
    a.sin6_port = htons((unsigned short)port);
    inet_pton(AF_INET6, addr, &a.sin6_addr);
    return a;
}

static struct sockaddr_in in4(const char *addr, int port) {
    struct sockaddr_in a;
    memset(&a, 0, sizeof a);
    a.sin_family = AF_INET;
    a.sin_port = htons((unsigned short)port);
    inet_pton(AF_INET, addr, &a.sin_addr);
    return a;
}

/* Whether `a` is IPv6 address `text`. */
static int is6(const struct sockaddr_in6 *a, const char *text) {
    struct in6_addr want;
    inet_pton(AF_INET6, text, &want);
    return a->sin6_family == AF_INET6 && memcmp(&a->sin6_addr, &want, sizeof want) == 0;
}

/* Wait for `fd` to be readable, at most `ms`. */
static int readable(int fd, int ms) {
    struct pollfd p = {fd, POLLIN, 0};
    return poll(&p, 1, ms) == 1;
}

static int interfaces(void) {
    struct ifaddrs *ifs, *p;
    unsigned net0 = if_nametoindex("net0");
    int lo6 = 0, ll = 0, slaac = 0;
    char ndb[512];
    int fd;
    ssize_t n;
    /* The router's advertisement comes within seconds of netd's start. */
    for (int tries = 0; tries < 150 && !slaac; tries++) {
        struct timespec ms = {0, 100000000};
        if (getifaddrs(&ifs) != 0) {
            return fail("getifaddrs");
        }
        lo6 = ll = slaac = 0;
        for (p = ifs; p != NULL; p = p->ifa_next) {
            const struct sockaddr_in6 *a = (const struct sockaddr_in6 *)p->ifa_addr;
            if (a == NULL || a->sin6_family != AF_INET6) {
                continue;
            }
            if (strcmp(p->ifa_name, "lo") == 0 && is6(a, "::1") && (p->ifa_flags & IFF_LOOPBACK)) {
                lo6 = 1;
            }
            if (strcmp(p->ifa_name, "net0") == 0 && IN6_IS_ADDR_LINKLOCAL(&a->sin6_addr)) {
                ll = a->sin6_scope_id == net0 && net0 == 2;
            }
            if (strcmp(p->ifa_name, "net0") == 0 && !IN6_IS_ADDR_LINKLOCAL(&a->sin6_addr)) {
                slaac = 1;
            }
        }
        freeifaddrs(ifs);
        if (!slaac) {
            nanosleep(&ms, NULL);
        }
    }
    if (!lo6) {
        return fail("lo has no ::1");
    }
    if (!ll) {
        return fail("net0 has no link-local address of its scope");
    }
    if (!slaac) {
        return fail("net0 has no SLAAC address");
    }
    fd = open("/net/ndb", O_RDONLY);
    n = fd < 0 ? -1 : read(fd, ndb, sizeof ndb - 1);
    if (fd >= 0) {
        close(fd);
    }
    ndb[n > 0 ? n : 0] = '\0';
    if (strstr(ndb, "ip6=") == NULL || strstr(ndb, "ip6gw=fe80:") == NULL) {
        printf("%s", ndb);
        return fail("/net/ndb has no ip6= line with its router");
    }
    return 0;
}

static int text(void) {
    static const char *forms[] = {"::1", "::", "fe80::5054:ff:fe12:3456", "2001:db8::1:0:0:1", "::ffff:10.0.2.2", "1:2:3:4:5:6:7:8"};
    for (unsigned i = 0; i < sizeof forms / sizeof forms[0]; i++) {
        struct in6_addr a;
        char back[INET6_ADDRSTRLEN];
        if (inet_pton(AF_INET6, forms[i], &a) != 1 || inet_ntop(AF_INET6, &a, back, sizeof back) == NULL
            || strcmp(back, forms[i]) != 0) {
            printf("[ FAIL ] ipv6 text %s -> %s\n", forms[i], back);
            return 1;
        }
    }
    {
        struct addrinfo hints, *res;
        char host[NI_MAXHOST];
        memset(&hints, 0, sizeof hints);
        hints.ai_family = AF_INET6;
        if (getaddrinfo("localhost", "80", &hints, &res) != 0 || res->ai_family != AF_INET6
            || !is6((struct sockaddr_in6 *)res->ai_addr, "::1")) {
            return fail("getaddrinfo(localhost, AF_INET6)");
        }
        freeaddrinfo(res);
        hints.ai_family = AF_UNSPEC;
        if (getaddrinfo("fec0::2", "80", &hints, &res) != 0 || res->ai_family != AF_INET6) {
            return fail("getaddrinfo of an IPv6 literal");
        }
        if (getnameinfo(res->ai_addr, res->ai_addrlen, host, sizeof host, NULL, 0, NI_NUMERICHOST) != 0
            || strcmp(host, "fec0::2") != 0) {
            return fail("getnameinfo");
        }
        freeaddrinfo(res);
    }
    return 0;
}

/* A TCP listener of family `family` on `addr` (`len`), v6only as asked. */
static int listener(int family, const void *addr, socklen_t len, int v6only) {
    int one = 1;
    int s = socket(family, SOCK_STREAM, 0);
    if (s < 0) {
        return -1;
    }
    setsockopt(s, SOL_SOCKET, SO_REUSEADDR, &one, sizeof one);
    if (family == AF_INET6 && setsockopt(s, IPPROTO_IPV6, IPV6_V6ONLY, &v6only, sizeof v6only) < 0) {
        close(s);
        return -1;
    }
    if (bind(s, (const struct sockaddr *)addr, len) < 0 || listen(s, 4) < 0) {
        int e = errno;
        close(s);
        errno = e;
        return -1;
    }
    return s;
}

static int tcp(void) {
    struct sockaddr_in6 any = in6("::", DUAL_PORT);
    struct sockaddr_in6 lo = in6("::1", TCP_PORT);
    struct sockaddr_in6 peer;
    socklen_t plen = sizeof peer;
    char buf[8];
    int l, c, a;
    /* ::1 to ::1. */
    if ((l = listener(AF_INET6, &lo, sizeof lo, 0)) < 0) {
        return fail("listen on [::1]");
    }
    c = socket(AF_INET6, SOCK_STREAM, 0);
    if (c < 0 || connect(c, (struct sockaddr *)&lo, sizeof lo) < 0) {
        return fail("connect to [::1]");
    }
    if ((a = accept(l, (struct sockaddr *)&peer, &plen)) < 0 || !is6(&peer, "::1")) {
        return fail("accept from ::1");
    }
    if (write(c, "ping", 4) != 4 || read(a, buf, sizeof buf) != 4 || memcmp(buf, "ping", 4) != 0
        || write(a, "pong", 4) != 4 || read(c, buf, sizeof buf) != 4 || memcmp(buf, "pong", 4) != 0) {
        return fail("talk over ::1");
    }
    plen = sizeof peer;
    if (getpeername(c, (struct sockaddr *)&peer, &plen) < 0 || !is6(&peer, "::1")
        || ntohs(peer.sin6_port) != TCP_PORT) {
        return fail("getpeername [::1]");
    }
    close(a);
    close(c);
    close(l);

    /* An IPv4 client of a listener on ::, which names it IPv4-mapped. */
    if ((l = listener(AF_INET6, &any, sizeof any, 0)) < 0) {
        return fail("listen on [::]");
    }
    {
        struct sockaddr_in lo4 = in4("127.0.0.1", DUAL_PORT);
        struct sockaddr_in any4 = in4("0.0.0.0", DUAL_PORT);
        int l4;
        c = socket(AF_INET, SOCK_STREAM, 0);
        if (c < 0 || connect(c, (struct sockaddr *)&lo4, sizeof lo4) < 0) {
            return fail("IPv4 connect to a listener on [::]");
        }
        plen = sizeof peer;
        if ((a = accept(l, (struct sockaddr *)&peer, &plen)) < 0 || !is6(&peer, "::ffff:127.0.0.1")) {
            return fail("accept of an IPv4 client on [::]");
        }
        close(a);
        close(c);
        /* Without v6only the IPv4 port is the listener's too. */
        l4 = listener(AF_INET, &any4, sizeof any4, 0);
        if (l4 >= 0 || errno != EADDRINUSE) {
            return fail("0.0.0.0 beside [::] without IPV6_V6ONLY");
        }
    }
    close(l);

    /* IPV6_V6ONLY: [::] and 0.0.0.0 on one port, each its own family's. */
    {
        struct sockaddr_in6 any6 = in6("::", SPLIT_PORT);
        struct sockaddr_in any4 = in4("0.0.0.0", SPLIT_PORT);
        struct sockaddr_in lo4 = in4("127.0.0.1", SPLIT_PORT);
        struct sockaddr_in6 lo6 = in6("::1", SPLIT_PORT);
        int l6 = listener(AF_INET6, &any6, sizeof any6, 1);
        int l4 = listener(AF_INET, &any4, sizeof any4, 0);
        int v6only = 0;
        socklen_t olen = sizeof v6only;
        if (l6 < 0 || l4 < 0) {
            return fail("[::] v6only beside 0.0.0.0");
        }
        if (getsockopt(l6, IPPROTO_IPV6, IPV6_V6ONLY, &v6only, &olen) < 0 || !v6only) {
            return fail("getsockopt IPV6_V6ONLY");
        }
        c = socket(AF_INET, SOCK_STREAM, 0);
        if (c < 0 || connect(c, (struct sockaddr *)&lo4, sizeof lo4) < 0) {
            return fail("IPv4 connect beside a v6only listener");
        }
        if (!readable(l4, 5000) || readable(l6, 0) || (a = accept(l4, NULL, NULL)) < 0) {
            return fail("the IPv4 listener takes the IPv4 client");
        }
        close(a);
        close(c);
        c = socket(AF_INET6, SOCK_STREAM, 0);
        if (c < 0 || connect(c, (struct sockaddr *)&lo6, sizeof lo6) < 0) {
            return fail("IPv6 connect to a v6only listener");
        }
        if (!readable(l6, 5000) || readable(l4, 0) || (a = accept(l6, NULL, NULL)) < 0) {
            return fail("the v6only listener takes the IPv6 client");
        }
        close(a);
        close(c);
        close(l6);
        close(l4);
    }
    return 0;
}

static int udp(void) {
    struct sockaddr_in6 lo = in6("::1", 0);
    struct sockaddr_in6 got, from;
    socklen_t len = sizeof got;
    char buf[16];
    int r = socket(AF_INET6, SOCK_DGRAM, 0);
    int s = socket(AF_INET6, SOCK_DGRAM, 0);
    if (r < 0 || s < 0 || bind(r, (struct sockaddr *)&lo, sizeof lo) < 0) {
        return fail("UDP bind [::1]:0");
    }
    if (getsockname(r, (struct sockaddr *)&got, &len) < 0 || !is6(&got, "::1") || got.sin6_port == 0) {
        return fail("UDP getsockname");
    }
    if (sendto(s, "dgram", 5, 0, (struct sockaddr *)&got, sizeof got) != 5) {
        return fail("UDP sendto [::1]");
    }
    len = sizeof from;
    if (!readable(r, 5000) || recvfrom(r, buf, sizeof buf, 0, (struct sockaddr *)&from, &len) != 5
        || memcmp(buf, "dgram", 5) != 0 || !is6(&from, "::1")) {
        return fail("UDP recvfrom ::1");
    }
    /* The answer to the sender, as it named itself. */
    if (sendto(r, "back", 4, 0, (struct sockaddr *)&from, len) != 4 || !readable(s, 5000)
        || recv(s, buf, sizeof buf, 0) != 4 || memcmp(buf, "back", 4) != 0) {
        return fail("UDP answer to ::1");
    }
    close(r);
    close(s);

    /* An IPv4 datagram to a socket on [::]. */
    {
        struct sockaddr_in6 any = in6("::", 0);
        struct sockaddr_in to;
        r = socket(AF_INET6, SOCK_DGRAM, 0);
        s = socket(AF_INET, SOCK_DGRAM, 0);
        len = sizeof got;
        if (r < 0 || s < 0 || bind(r, (struct sockaddr *)&any, sizeof any) < 0
            || getsockname(r, (struct sockaddr *)&got, &len) < 0) {
            return fail("UDP bind [::]:0");
        }
        to = in4("127.0.0.1", ntohs(got.sin6_port));
        if (sendto(s, "four", 4, 0, (struct sockaddr *)&to, sizeof to) != 4) {
            return fail("UDP sendto 127.0.0.1");
        }
        len = sizeof from;
        if (!readable(r, 5000) || recvfrom(r, buf, sizeof buf, 0, (struct sockaddr *)&from, &len) != 4
            || !is6(&from, "::ffff:127.0.0.1")) {
            return fail("UDP IPv4 datagram on [::]");
        }
        close(r);
        close(s);
    }

    /* A port nobody bound: ICMPv6's port unreachable refuses the next send. */
    {
        struct sockaddr_in6 closed = in6("::1", CLOSED_PORT);
        int refused = 0;
        s = socket(AF_INET6, SOCK_DGRAM, 0);
        if (s < 0 || connect(s, (struct sockaddr *)&closed, sizeof closed) < 0) {
            return fail("UDP connect [::1]");
        }
        for (int i = 0; i < 50 && !refused; i++) {
            struct timespec ms = {0, 20000000};
            refused = send(s, "x", 1, 0) < 0 && errno == ECONNREFUSED;
            nanosleep(&ms, NULL);
        }
        if (!refused) {
            return fail("UDP to a closed port of ::1 not refused");
        }
        close(s);
    }
    return 0;
}

/* An echo to `addr` over /net/icmp comes back. */
static int echo(const char *addr) {
    char path[64], id[16], buf[64], cmd[80];
    int data = open("/net/icmp/clone", O_RDWR);
    int ctl;
    ssize_t n;
    if (data < 0) {
        return fail("/net/icmp/clone");
    }
    snprintf(path, sizeof path, "/proc/self/fd/%d", data);
    n = readlink(path, buf, sizeof buf - 1);
    if (n <= 0) {
        return fail("readlink of the icmp conversation");
    }
    buf[n] = '\0';
    if (sscanf(buf, "/net/icmp/%15[0-9]/data", id) != 1) {
        return fail("icmp conversation's name");
    }
    snprintf(path, sizeof path, "/net/icmp/%s/ctl", id);
    snprintf(cmd, sizeof cmd, "connect %s", addr);
    ctl = open(path, O_RDWR);
    if (ctl < 0 || write(ctl, cmd, strlen(cmd)) != (ssize_t)strlen(cmd)) {
        return fail("icmp connect");
    }
    /* The first may go before the router's address is resolved. */
    for (int i = 0; i < 5; i++) {
        if (write(data, "echo6", 5) != 5) {
            return fail("icmp echo");
        }
        if (readable(data, 2000) && read(data, buf, sizeof buf) == 5 && memcmp(buf, "echo6", 5) == 0) {
            close(ctl);
            close(data);
            return 0;
        }
    }
    printf("[ FAIL ] ipv6 no echo reply from %s\n", addr);
    return 1;
}

/* Echoes from ::1 and from the router of /net/ndb's ip6gw=. */
static int icmp(void) {
    char ndb[512], gw[INET6_ADDRSTRLEN];
    const char *p;
    int fd = open("/net/ndb", O_RDONLY);
    ssize_t n = fd < 0 ? -1 : read(fd, ndb, sizeof ndb - 1);
    if (fd >= 0) {
        close(fd);
    }
    ndb[n > 0 ? n : 0] = '\0';
    if ((p = strstr(ndb, "ip6gw=")) == NULL || sscanf(p + 6, "%45[0-9a-f:]", gw) != 1) {
        return fail("/net/ndb's ip6gw=");
    }
    return echo("::1") || echo(gw);
}

/* Whether /net/ndb's first DNS server has DNS_NAME's AAAA record, asked
 * over IPv4 (1 yes, 0 no, -1 no answer). */
static int resolver_has_aaaa(void) {
    static const unsigned char name[] = "\7example\3com";
    unsigned char q[64], a[512];
    char ndb[512], ns[16] = "10.0.2.3";
    struct sockaddr_in server;
    size_t len = 12;
    ssize_t got = -1;
    char *p;
    int s;
    int fd = open("/net/ndb", O_RDONLY);
    ssize_t n = fd < 0 ? -1 : read(fd, ndb, sizeof ndb - 1);
    if (fd >= 0) {
        close(fd);
    }
    ndb[n > 0 ? n : 0] = '\0';
    if ((p = strstr(ndb, "dns=")) != NULL) {
        sscanf(p + 4, "%15[0-9.]", ns);
    }
    memset(&server, 0, sizeof server);
    server.sin_family = AF_INET;
    server.sin_port = htons(53);
    inet_pton(AF_INET, ns, &server.sin_addr);
    memset(q, 0, sizeof q);
    q[0] = 0x6d;
    q[1] = 0x79;
    q[2] = 0x01; /* recursion desired */
    q[5] = 1;    /* one question */
    memcpy(q + len, name, sizeof name);
    len += sizeof name;
    q[len++] = 0;
    q[len++] = 28; /* AAAA */
    q[len++] = 0;
    q[len++] = 1; /* IN */
    s = socket(AF_INET, SOCK_DGRAM, 0);
    if (s < 0 || connect(s, (struct sockaddr *)&server, sizeof server) < 0) {
        return -1;
    }
    for (int i = 0; i < 4 && got < 12; i++) {
        if (send(s, q, len, 0) == (ssize_t)len && readable(s, 5000)) {
            got = recv(s, a, sizeof a, 0);
        }
    }
    close(s);
    if (got < 12 || a[0] != 0x6d || a[1] != 0x79) {
        return -1;
    }
    return (a[3] & 0x0f) == 0 && (a[6] | a[7]) != 0;
}

/* getaddrinfo(AF_INET6) finds DNS_NAME's AAAA record and AF_UNSPEC lists
 * its IPv4 addresses first, then the IPv6 ones, when the resolver has the
 * record (a host's may not: this sandbox's has no AAAA records at all;
 * QEMU advertises its IPv6 resolver, fec0::3, only when the host has an
 * IPv6 nameserver, so the lookups go to the IPv4 one). */
static int dns(void) {
    struct addrinfo hints, *res;
    int has = resolver_has_aaaa();
    if (has < 0) {
        return fail("no answer from /net/ndb's DNS server");
    }
    if (!has) {
        printf("ipv6 dns: the resolver has no AAAA record for " DNS_NAME "; skipped\n");
        return 0;
    }
    memset(&hints, 0, sizeof hints);
    hints.ai_family = AF_INET6;
    if (getaddrinfo(DNS_NAME, "80", &hints, &res) != 0 || res->ai_family != AF_INET6) {
        return fail("getaddrinfo(" DNS_NAME ", AF_INET6)");
    }
    freeaddrinfo(res);
#ifndef __MLIBC_POSIX_OPTION
    /* Not on mlibc (its headers define the __MLIBC_* options): its
     * AF_UNSPEC lookup asks for A records alone. */
    hints.ai_family = AF_UNSPEC;
    hints.ai_socktype = SOCK_STREAM;
    if (getaddrinfo(DNS_NAME, "80", &hints, &res) != 0 || res->ai_family != AF_INET) {
        return fail("getaddrinfo(" DNS_NAME ", AF_UNSPEC): IPv4 first");
    }
    for (struct addrinfo *r = res; r != NULL; r = r->ai_next) {
        if (r->ai_family == AF_INET6) {
            freeaddrinfo(res);
            return 0;
        }
    }
    freeaddrinfo(res);
    return fail("getaddrinfo(" DNS_NAME ", AF_UNSPEC): no IPv6 address");
#else
    return 0;
#endif
}

#ifndef __MLIBC_POSIX_OPTION
/* The test's own DNS server, on 127.0.0.1:FAKE_DNS_PORT. */
#define FAKE_DNS_PORT 5553
#define FAKE_NAME "retry.test"
static const unsigned char fake_a[4] = {192, 0, 2, 7};
static const unsigned char fake_aaaa[16] = {0x20, 0x01, 0x0d, 0xb8, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 7};

/* Serve the queries on `sock` until killed: an A query gets fake_a at
 * once, the first AAAA query no answer (the datagram lost), the next ones
 * fake_aaaa. */
static void fake_dns(int sock) {
    unsigned char q[512], a[560];
    struct sockaddr_in from;
    socklen_t fromlen;
    int dropped = 0;
    for (;;) {
        ssize_t n;
        size_t pos = 12, len;
        unsigned type;
        fromlen = sizeof from;
        n = recvfrom(sock, q, sizeof q, 0, (struct sockaddr *)&from, &fromlen);
        if (n < 12) {
            continue;
        }
        while (pos < (size_t)n && q[pos] != 0) {
            pos += 1 + q[pos];
        }
        pos++;
        if (pos + 4 > (size_t)n) {
            continue;
        }
        type = ((unsigned)q[pos] << 8) | q[pos + 1];
        if (type == 28 && !dropped) {
            dropped = 1;
            continue;
        }
        memcpy(a, q, (size_t)n);
        a[2] |= 0x80; /* a response */
        a[7] = 1;     /* one answer */
        len = (size_t)n;
        a[len++] = 0xc0; /* the question's name */
        a[len++] = 0x0c;
        a[len++] = (unsigned char)(type >> 8);
        a[len++] = (unsigned char)type;
        a[len++] = 0; /* IN */
        a[len++] = 1;
        a[len++] = 0; /* TTL 60 s */
        a[len++] = 0;
        a[len++] = 0;
        a[len++] = 60;
        a[len++] = 0;
        if (type == 28) {
            a[len++] = 16;
            memcpy(a + len, fake_aaaa, 16);
            len += 16;
        } else {
            a[len++] = 4;
            memcpy(a + len, fake_a, 4);
            len += 4;
        }
        sendto(sock, a, len, 0, (struct sockaddr *)&from, fromlen);
    }
}

/* getaddrinfo(AF_UNSPEC) against fake_dns gives the A address first and
 * the AAAA one too, within a few seconds (the AAAA query sent again a
 * second after the A answer, not after the 5 s a server gets). */
static int dns_retry(void) {
    struct sockaddr_in me;
    struct addrinfo hints, *res, *r;
    struct timespec t0, t1;
    long ms;
    int sock, v4 = 0, v6 = 0, rc;
    pid_t pid;
    sock = socket(AF_INET, SOCK_DGRAM, 0);
    memset(&me, 0, sizeof me);
    me.sin_family = AF_INET;
    me.sin_port = htons(FAKE_DNS_PORT);
    me.sin_addr.s_addr = htonl(INADDR_LOOPBACK);
    if (sock < 0 || bind(sock, (struct sockaddr *)&me, sizeof me) != 0) {
        return fail("dnsretry: the fake server's socket");
    }
    pid = fork();
    if (pid < 0) {
        return fail("dnsretry: fork");
    }
    if (pid == 0) {
        fake_dns(sock);
        _exit(0);
    }
    close(sock);
    setenv("MYOS_DNS", "127.0.0.1!5553", 1);
    memset(&hints, 0, sizeof hints);
    hints.ai_family = AF_UNSPEC;
    hints.ai_socktype = SOCK_STREAM;
    clock_gettime(CLOCK_MONOTONIC, &t0);
    rc = getaddrinfo(FAKE_NAME, "80", &hints, &res);
    clock_gettime(CLOCK_MONOTONIC, &t1);
    unsetenv("MYOS_DNS");
    kill(pid, SIGTERM);
    waitpid(pid, NULL, 0);
    ms = (t1.tv_sec - t0.tv_sec) * 1000L + (t1.tv_nsec - t0.tv_nsec) / 1000000L;
    if (rc != 0) {
        printf("[ FAIL ] ipv6 dnsretry: getaddrinfo: %s after %ld ms\n", gai_strerror(rc), ms);
        return 1;
    }
    for (r = res; r != NULL; r = r->ai_next) {
        if (r->ai_family == AF_INET && !v6
            && memcmp(&((struct sockaddr_in *)r->ai_addr)->sin_addr, fake_a, 4) == 0) {
            v4 = 1;
        } else if (r->ai_family == AF_INET6
            && memcmp(((struct sockaddr_in6 *)r->ai_addr)->sin6_addr.s6_addr, fake_aaaa, 16) == 0) {
            v6 = 1;
        }
    }
    freeaddrinfo(res);
    if (!v4 || !v6) {
        printf("[ FAIL ] ipv6 dnsretry: the lookup gave %s%s after %ld ms\n", v4 ? "IPv4" : "",
               v6 ? (v4 ? " and IPv6" : "IPv6") : (v4 ? " alone" : "nothing"), ms);
        return 1;
    }
    if (ms > 4000) {
        printf("[ FAIL ] ipv6 dnsretry: the lookup took %ld ms\n", ms);
        return 1;
    }
    return 0;
}
#endif

int main(int argc, char **argv) {
    if (argc > 1 && strcmp(argv[1], "dnsretry") == 0) {
#ifndef __MLIBC_POSIX_OPTION
        if (dns_retry()) {
            return 1;
        }
#endif
        printf("[ OK ] ipv6 dnsretry\n");
        return 0;
    }
    if (argc > 1 && strcmp(argv[1], "dns") == 0) {
        if (interfaces() || dns()) {
            return 1;
        }
        printf("[ OK ] ipv6 dns\n");
        return 0;
    }
    if (interfaces() || text() || tcp() || udp() || icmp()) {
        return 1;
    }
    printf("[ OK ] ipv6\n");
    return 0;
}
