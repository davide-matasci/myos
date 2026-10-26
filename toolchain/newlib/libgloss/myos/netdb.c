/*
 * getaddrinfo / gethostbyname via Plan 9 /net/udp to the DNS servers of
 * netd's lease and router advertisement (`dns=` in /net/ndb, IPv4 or
 * IPv6), QEMU's 10.0.2.3 when it names none. Its A lookups mirror
 * user/lib/src/dns.rs — keep behaviour in sync.
 *
 * AF_UNSPEC asks for A and AAAA records at once (two queries on one
 * socket, as glibc does) and lists the IPv4 addresses first: QEMU's user
 * network gives the guest an IPv6 address whatever the host can reach,
 * and a program that tries the first address alone would try IPv6 first
 * (RFC 6724 too prefers IPv4 to a site-local IPv6 source). AAAA records
 * are asked for only while the host has an IPv6 address beyond ::1 and
 * the link-local ones (AI_ADDRCONFIG's rule).
 */
#include <arpa/inet.h>
#include <errno.h>
#include <fcntl.h>
#include <ifaddrs.h>
#include <netdb.h>
#include <netinet/in.h>
#include <poll.h>
#include "myos_fmt.h"
#include <stdlib.h>
#include <string.h>
#include <sys/socket.h>
#include <sys/time.h>
#include <unistd.h>

/* How long one DNS server gets to answer. */
#define DNS_TIMEOUT_MS 5000
/* Once one of two queries is answered, how long the other one may take. */
#define DNS_SECOND_MS 1000
/* A query is one datagram, lost at times, and the CI runner's resolver has
 * taken over 5 s for a name's first lookup: an unanswered query is sent
 * again, this many times in all (musl's resolver retransmits too). */
#define DNS_ATTEMPTS 4
#define DNS_BUF      1024
#define MAX_DNS      3
#define NDB_BUF      512
#define TYPE_A       1
#define TYPE_AAAA    28
/* Addresses one lookup keeps of each version. */
#define MAX_ADDRS    4

/* A DNS server's address. */
struct server {
    int v6;
    unsigned char addr[16];
};

/* The addresses a lookup found, of one version. */
struct found {
    int n;
    unsigned char addr[MAX_ADDRS][16];
};

static int encode_name(unsigned char *dst, size_t dstcap, const char *name) {
    size_t pos = 0;
    const char *start = name;
    while (*start) {
        const char *end = start;
        size_t len;
        while (*end && *end != '.') {
            end++;
        }
        len = (size_t)(end - start);
        if (len == 0 || len > 63 || pos + 1 + len >= dstcap) {
            return 0;
        }
        dst[pos++] = (unsigned char)len;
        memcpy(dst + pos, start, len);
        pos += len;
        if (*end == '\0') {
            break;
        }
        start = end + 1;
    }
    if (pos >= dstcap) {
        return 0;
    }
    dst[pos++] = 0;
    return (int)pos;
}

/* The records of `type` (4 bytes for A, 16 for AAAA) of the answer in
 * `buf` into `out` (none for a name without them). -1 for a malformed
 * answer. */
static int parse_records(const unsigned char *buf, size_t len, unsigned type, struct found *out) {
    size_t pos = 12;
    unsigned ancount;
    unsigned i;
    unsigned want = type == TYPE_A ? 4 : 16;

    out->n = 0;
    if (len < 12) {
        return -1;
    }
    for (;;) {
        if (pos >= len) {
            return -1;
        }
        if ((buf[pos] & 0xC0) == 0xC0) {
            pos += 2;
            break;
        }
        if (buf[pos] == 0) {
            pos += 1;
            break;
        }
        pos += 1 + buf[pos];
    }
    if (pos + 4 > len) {
        return -1;
    }
    pos += 4; /* QTYPE + QCLASS */
    ancount = ((unsigned)buf[6] << 8) | buf[7];
    for (i = 0; i < ancount && pos + 12 <= len; i++) {
        unsigned rtype, rdlen;
        if ((buf[pos] & 0xC0) == 0xC0) {
            pos += 2;
        } else {
            while (pos < len && buf[pos] != 0) {
                if ((buf[pos] & 0xC0) == 0xC0) {
                    pos += 2;
                    break;
                }
                pos += 1 + buf[pos];
            }
            if (pos < len && buf[pos] == 0) {
                pos++;
            }
        }
        if (pos + 10 > len) {
            return -1;
        }
        rtype = ((unsigned)buf[pos] << 8) | buf[pos + 1];
        rdlen = ((unsigned)buf[pos + 8] << 8) | buf[pos + 9];
        pos += 10;
        if (pos + rdlen > len) {
            return -1;
        }
        if (rtype == type && rdlen == want && out->n < MAX_ADDRS) {
            memcpy(out->addr[out->n++], buf + pos, want);
        }
        pos += rdlen;
    }
    return 0;
}

/* The `dns=` servers of /net/ndb (at most `max`), or QEMU's 10.0.2.3. */
static int dns_servers(struct server *out, int max) {
    char ndb[NDB_BUF + 1];
    ssize_t n = 0;
    int fd = open("/net/ndb", O_RDONLY);
    int count = 0;
    char *p;
    if (fd >= 0) {
        n = read(fd, ndb, NDB_BUF);
        close(fd);
    }
    ndb[n > 0 ? n : 0] = '\0';
    for (p = strstr(ndb, "dns="); p != NULL && count < max; p = strstr(p, "dns=")) {
        char addr[INET6_ADDRSTRLEN];
        size_t len;
        p += 4;
        len = strcspn(p, " \t\n");
        if (len < sizeof addr) {
            memcpy(addr, p, len);
            addr[len] = '\0';
            memset(&out[count], 0, sizeof out[count]);
            if (inet_pton(AF_INET, addr, out[count].addr) == 1) {
                count++;
            } else if (inet_pton(AF_INET6, addr, out[count].addr) == 1) {
                out[count++].v6 = 1;
            }
        }
    }
    if (count == 0) {
        memset(&out[0], 0, sizeof out[0]);
        inet_pton(AF_INET, "10.0.2.3", out[0].addr);
        count = 1;
    }
    return count;
}

static long elapsed_ms(const struct timeval *start) {
    struct timeval now;
    if (gettimeofday(&now, NULL) != 0) {
        return 0;
    }
    return (now.tv_sec - start->tv_sec) * 1000L + (now.tv_usec - start->tv_usec) / 1000L;
}

/* Query `server` for the `ntypes` record types of `types` about `name`, at
 * once, into `out` (one per type). The first answer to a query is the
 * server's word for it, records or none: 0 once each query had one, or
 * once one had and the others had DNS_SECOND_MS more; -1 when none was
 * answered. */
static int query_server(const struct server *server, const char *name,
    const unsigned *types, int ntypes, struct found *out) {
    unsigned char query[2][300];
    size_t qlen[2];
    int answered[2] = {0, 0};
    int got = 0;
    unsigned char rbuf[DNS_BUF];
    union {
        struct sockaddr_in in;
        struct sockaddr_in6 in6;
    } dest;
    socklen_t destlen;
    struct timeval start;
    ssize_t nr;
    int sock;
    int attempt;
    int k;

    for (k = 0; k < ntypes; k++) {
        int name_len;
        unsigned char *q = query[k];
        memset(q, 0, sizeof query[k]);
        q[0] = 0x12;
        q[1] = (unsigned char)(0x34 + k); /* the id: which query an answer is */
        q[2] = 0x01;
        q[5] = 0x01;
        name_len = encode_name(q + 12, sizeof query[k] - 16, name);
        if (name_len == 0) {
            return -1;
        }
        qlen[k] = 12 + (size_t)name_len;
        q[qlen[k]++] = (unsigned char)(types[k] >> 8);
        q[qlen[k]++] = (unsigned char)types[k];
        q[qlen[k]++] = 0x00;
        q[qlen[k]++] = 0x01; /* IN */
        out[k].n = 0;
    }
    sock = socket(server->v6 ? AF_INET6 : AF_INET, SOCK_DGRAM, 0);
    if (sock < 0) {
        return -1;
    }
    memset(&dest, 0, sizeof dest);
    if (server->v6) {
        dest.in6.sin6_family = AF_INET6;
        dest.in6.sin6_port = htons(53);
        memcpy(dest.in6.sin6_addr.s6_addr, server->addr, 16);
        destlen = sizeof dest.in6;
    } else {
        dest.in.sin_family = AF_INET;
        dest.in.sin_port = htons(53);
        memcpy(&dest.in.sin_addr, server->addr, 4);
        destlen = sizeof dest.in;
    }
    if (connect(sock, (struct sockaddr *)&dest, destlen) < 0) {
        close(sock);
        return -1;
    }
    /* Wait for the answers in the kernel's poll (DNS_TIMEOUT_MS at most per
     * attempt), not by reading the socket again and again: that spun for
     * minutes, hogging a CPU and netd, when the server had nothing to say. */
    (void)fcntl(sock, F_SETFL, O_NONBLOCK);
    for (attempt = 0; attempt < DNS_ATTEMPTS && got < ntypes; attempt++) {
        long limit = DNS_TIMEOUT_MS;
        for (k = 0; k < ntypes; k++) {
            if (!answered[k] && send(sock, query[k], qlen[k], 0) < 0) {
                close(sock);
                return -1;
            }
        }
        if (gettimeofday(&start, NULL) != 0) {
            close(sock);
            return -1;
        }
        while (got < ntypes) {
            struct pollfd p;
            long left = limit - elapsed_ms(&start);
            int r;
            if (left <= 0) {
                break;
            }
            p.fd = sock;
            p.events = POLLIN;
            p.revents = 0;
            r = poll(&p, 1, (int)left);
            if (r < 0 && errno == EINTR) {
                continue;
            }
            if (r < 0) {
                close(sock);
                return -1;
            }
            if (r == 0) {
                break;
            }
            nr = recv(sock, rbuf, sizeof rbuf, 0);
            if (nr < 0) {
                if (errno == EAGAIN || errno == EWOULDBLOCK) {
                    continue;
                }
                close(sock);
                return -1;
            }
            if (nr < 12 || rbuf[0] != 0x12) {
                continue;
            }
            k = rbuf[1] - 0x34;
            if (k < 0 || k >= ntypes || answered[k]) {
                continue;
            }
            if (parse_records(rbuf, (size_t)nr, types[k], &out[k]) < 0) {
                out[k].n = 0;
            }
            answered[k] = 1;
            got++;
            /* The other one gets a little longer, no more. */
            limit = elapsed_ms(&start) + DNS_SECOND_MS;
        }
        if (got > 0) {
            break;
        }
    }
    close(sock);
    return got > 0 ? 0 : -1;
}

/* Whether the host has an IPv6 address beyond ::1 and the link-local ones
 * (AI_ADDRCONFIG's rule for asking for AAAA records). */
static int have_ipv6(void) {
    struct ifaddrs *ifs, *p;
    int found = 0;
    if (getifaddrs(&ifs) != 0) {
        return 0;
    }
    for (p = ifs; p != NULL; p = p->ifa_next) {
        const unsigned char *a;
        if (p->ifa_addr == NULL || p->ifa_addr->sa_family != AF_INET6) {
            continue;
        }
        a = ((struct sockaddr_in6 *)p->ifa_addr)->sin6_addr.s6_addr;
        if (!(a[0] == 0xfe && (a[1] & 0xc0) == 0x80) && !IN6_IS_ADDR_LOOPBACK((const struct in6_addr *)a)) {
            found = 1;
        }
    }
    freeifaddrs(ifs);
    return found;
}

/* The addresses of `host` of the versions asked (`v4`, `v6`): a literal,
 * localhost (the loopback address, as /etc/hosts says elsewhere: no DNS
 * server knows the name, and an X client that found no unix socket tries
 * localhost:6000 next, and waited on that lookup), or the A and AAAA
 * records of the first server that answers. 0 when one was found. */
static int resolve(const char *host, int v4, int v6, struct found *out4, struct found *out6) {
    struct server servers[MAX_DNS];
    unsigned types[2];
    struct found found[2];
    int ntypes = 0;
    int nservers;
    int i;

    out4->n = 0;
    out6->n = 0;
    if (inet_pton(AF_INET, host, out4->addr[0]) == 1) {
        out4->n = v4;
        return v4 ? 0 : -1;
    }
    if (inet_pton(AF_INET6, host, out6->addr[0]) == 1) {
        out6->n = v6;
        return v6 ? 0 : -1;
    }
    if (strcmp(host, "localhost") == 0) {
        static const unsigned char lo4[4] = {127, 0, 0, 1};
        static const unsigned char lo6[16] = {0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1};
        memcpy(out4->addr[0], lo4, 4);
        memcpy(out6->addr[0], lo6, 16);
        out4->n = v4;
        out6->n = v6;
        return 0;
    }
    if (v4) {
        types[ntypes++] = TYPE_A;
    }
    if (v6) {
        types[ntypes++] = TYPE_AAAA;
    }
    nservers = dns_servers(servers, MAX_DNS);
    for (i = 0; i < nservers; i++) {
        if (query_server(&servers[i], host, types, ntypes, found) == 0) {
            for (int k = 0; k < ntypes; k++) {
                *(types[k] == TYPE_A ? out4 : out6) = found[k];
            }
            return out4->n + out6->n > 0 ? 0 : -1;
        }
    }
    return -1;
}

static int parse_port(const char *service, unsigned short *port) {
    unsigned long v = 0;
    const char *p;
    if (service == NULL || *service == '\0') {
        *port = 0;
        return 0;
    }
    /* numeric */
    for (p = service; *p; p++) {
        if (*p < '0' || *p > '9') {
            break;
        }
        v = v * 10ul + (unsigned long)(*p - '0');
        if (v > 65535) {
            return -1;
        }
    }
    if (*p == '\0') {
        *port = (unsigned short)v;
        return 0;
    }
    if (strcmp(service, "http") == 0) {
        *port = 80;
        return 0;
    }
    if (strcmp(service, "https") == 0) {
        *port = 443;
        return 0;
    }
    if (strcmp(service, "domain") == 0 || strcmp(service, "dns") == 0) {
        *port = 53;
        return 0;
    }
    return -1;
}

/* An addrinfo for address `a` of family `family` behind `*tail`. */
static int add_ai(struct addrinfo ***tail, int family, const unsigned char *a, unsigned short port,
    int socktype, int protocol) {
    union {
        struct sockaddr_in in;
        struct sockaddr_in6 in6;
    } *sa;
    struct addrinfo *ai = (struct addrinfo *)calloc(1, sizeof(*ai) + sizeof(*sa));
    if (ai == NULL) {
        return -1;
    }
    sa = (void *)(ai + 1);
    if (family == AF_INET6) {
        sa->in6.sin6_family = AF_INET6;
        sa->in6.sin6_port = htons(port);
        memcpy(sa->in6.sin6_addr.s6_addr, a, 16);
        ai->ai_addrlen = sizeof sa->in6;
    } else {
        sa->in.sin_family = AF_INET;
        sa->in.sin_port = htons(port);
        memcpy(&sa->in.sin_addr, a, 4);
        ai->ai_addrlen = sizeof sa->in;
    }
    ai->ai_family = family;
    ai->ai_socktype = socktype;
    ai->ai_protocol = protocol;
    ai->ai_addr = (struct sockaddr *)sa;
    **tail = ai;
    *tail = &ai->ai_next;
    return 0;
}

int getaddrinfo(const char *node, const char *service,
    const struct addrinfo *hints, struct addrinfo **res) {
    struct addrinfo **tail = res;
    struct found found4, found6;
    unsigned short port = 0;
    int family = AF_UNSPEC;
    int socktype = SOCK_STREAM;
    int protocol = IPPROTO_TCP;
    int v4, v6;
    int k;

    if (res == NULL) {
        return EAI_SYSTEM;
    }
    *res = NULL;

    if (hints) {
        family = hints->ai_family;
        if (family != AF_UNSPEC && family != AF_INET && family != AF_INET6) {
            return EAI_FAMILY;
        }
        if (hints->ai_socktype == SOCK_DGRAM) {
            socktype = SOCK_DGRAM;
            protocol = IPPROTO_UDP;
        } else if (hints->ai_socktype == SOCK_STREAM || hints->ai_socktype == 0) {
            socktype = SOCK_STREAM;
            protocol = IPPROTO_TCP;
        } else {
            return EAI_SOCKTYPE;
        }
    }

    if (parse_port(service, &port) < 0) {
        return EAI_SERVICE;
    }

    v4 = family != AF_INET6;
    v6 = family == AF_INET6;
    if (node == NULL || node[0] == '\0') {
        /* The any or the loopback address: IPv4's unless asked for IPv6. */
        int passive = hints && (hints->ai_flags & AI_PASSIVE);
        memset(&found4, 0, sizeof found4);
        memset(&found6, 0, sizeof found6);
        found4.n = v4;
        found6.n = v6;
        if (!passive) {
            found4.addr[0][0] = 127;
            found4.addr[0][3] = 1;
            found6.addr[0][15] = 1;
        }
    } else {
        int numeric = hints && (hints->ai_flags & AI_NUMERICHOST);
        struct in6_addr tmp;
        if (family == AF_UNSPEC) {
            v6 = inet_pton(AF_INET6, node, &tmp) == 1 || strcmp(node, "localhost") == 0
                || (!numeric && have_ipv6());
        }
        if (numeric && inet_pton(AF_INET, node, &tmp) != 1 && inet_pton(AF_INET6, node, &tmp) != 1) {
            return EAI_NONAME;
        }
        if (resolve(node, v4, v6, &found4, &found6) < 0) {
            return EAI_NONAME;
        }
    }

    for (k = 0; k < found4.n; k++) {
        if (add_ai(&tail, AF_INET, found4.addr[k], port, socktype, protocol) < 0) {
            freeaddrinfo(*res);
            *res = NULL;
            return EAI_MEMORY;
        }
    }
    for (k = 0; k < found6.n; k++) {
        if (add_ai(&tail, AF_INET6, found6.addr[k], port, socktype, protocol) < 0) {
            freeaddrinfo(*res);
            *res = NULL;
            return EAI_MEMORY;
        }
    }
    return *res != NULL ? 0 : EAI_NONAME;
}

void freeaddrinfo(struct addrinfo *res) {
    while (res) {
        struct addrinfo *n = res->ai_next;
        free(res);
        res = n;
    }
}

const char *gai_strerror(int errcode) {
    switch (errcode) {
    case 0: return "Success";
    case EAI_AGAIN: return "Temporary failure in name resolution";
    case EAI_FAIL: return "Non-recoverable failure in name resolution";
    case EAI_FAMILY: return "ai_family not supported";
    case EAI_MEMORY: return "Memory allocation failure";
    case EAI_NONAME: return "Name or service not known";
    case EAI_SERVICE: return "Servname not supported for ai_socktype";
    case EAI_SOCKTYPE: return "ai_socktype not supported";
    case EAI_SYSTEM: return "System error";
    default: return "Unknown error";
    }
}

/* Numeric only: there is no reverse lookup. */
int getnameinfo(const struct sockaddr *sa, socklen_t salen,
    char *host, socklen_t hostlen, char *serv, socklen_t servlen, int flags) {
    const void *addr;
    unsigned short port;
    (void)flags;
    if (sa != NULL && sa->sa_family == AF_INET && salen >= sizeof(struct sockaddr_in)) {
        addr = &((const struct sockaddr_in *)sa)->sin_addr;
        port = ((const struct sockaddr_in *)sa)->sin_port;
    } else if (sa != NULL && sa->sa_family == AF_INET6 && salen >= sizeof(struct sockaddr_in6)) {
        addr = &((const struct sockaddr_in6 *)sa)->sin6_addr;
        port = ((const struct sockaddr_in6 *)sa)->sin6_port;
    } else {
        return EAI_FAMILY;
    }
    if (host && hostlen > 0) {
        if (inet_ntop(sa->sa_family, addr, host, hostlen) == NULL) {
            return EAI_OVERFLOW;
        }
    }
    if (serv && servlen > 0) {
        if (myos_u16_dec(serv, servlen, ntohs(port)) < 0) {
            return EAI_OVERFLOW;
        }
    }
    return 0;
}

struct hostent *gethostbyname(const char *name) {
    static struct hostent he;
    static char *aliases[1];
    static char *addr_list[2];
    static char addr[4];
    static char hostname[256];

    struct found found4, found6;
    if (name == NULL || resolve(name, 1, 0, &found4, &found6) < 0 || found4.n == 0) {
        return NULL;
    }
    memcpy(addr, found4.addr[0], 4);
    strncpy(hostname, name, sizeof hostname - 1);
    hostname[sizeof hostname - 1] = '\0';
    aliases[0] = NULL;
    addr_list[0] = addr;
    addr_list[1] = NULL;
    he.h_name = hostname;
    he.h_aliases = aliases;
    he.h_addrtype = AF_INET;
    he.h_length = 4;
    he.h_addr_list = addr_list;
    return &he;
}

int h_errno;

const char *hstrerror(int err) {
    switch (err) {
    case 0:
        return "Resolver error 0 (no error)";
    case HOST_NOT_FOUND:
        return "Unknown host";
    case TRY_AGAIN:
        return "Host name lookup failure";
    case NO_RECOVERY:
        return "Unknown server error";
    case NO_DATA:
        return "No address associated with name";
    default:
        return "Unknown resolver error";
    }
}

/* No services database: a lookup by name or port finds nothing, and a
 * caller falls back to the number it has (xtrans, git's daemon). */
struct servent *getservbyname(const char *name, const char *proto) {
    (void)name;
    (void)proto;
    return NULL;
}

struct servent *getservbyport(int port, const char *proto) {
    (void)port;
    (void)proto;
    return NULL;
}
