/*
 * getaddrinfo / gethostbyname via Plan 9 /net/udp to QEMU DNS (10.0.2.3:53).
 * Logic mirrors user/lib/src/dns.rs — keep behaviour in sync.
 */
#include <arpa/inet.h>
#include <errno.h>
#include <fcntl.h>
#include <netdb.h>
#include <netinet/in.h>
#include "myos_fmt.h"
#include <stdlib.h>
#include <string.h>
#include <sys/socket.h>
#include <unistd.h>

#define STATUS_POLLS 400000
#define DATA_POLLS   400000
#define DNS_BUF      1024

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

static int parse_a_record(const unsigned char *buf, size_t len, unsigned char out[4]) {
    size_t pos = 12;
    size_t question_end;
    unsigned ancount;
    unsigned i;

    if (len < 12 + 16) {
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
    question_end = pos;
    ancount = ((unsigned)buf[6] << 8) | buf[7];
    pos = question_end;
    for (i = 0; i < ancount && pos + 12 <= len; i++) {
        unsigned type, rdlen;
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
        type = ((unsigned)buf[pos] << 8) | buf[pos + 1];
        rdlen = ((unsigned)buf[pos + 8] << 8) | buf[pos + 9];
        pos += 10;
        if (pos + rdlen > len) {
            return -1;
        }
        if (type == 1 && rdlen == 4) {
            memcpy(out, buf + pos, 4);
            return 0;
        }
        pos += rdlen;
    }
    return -1;
}

static int buf_has(const char *hay, size_t n, const char *needle) {
    size_t m = strlen(needle);
    size_t i;
    if (m == 0 || n < m) {
        return 0;
    }
    for (i = 0; i + m <= n; i++) {
        if (memcmp(hay + i, needle, m) == 0) {
            return 1;
        }
    }
    return 0;
}

static int resolve_a(const char *host, unsigned char ip[4]) {
    unsigned char query[512];
    size_t qpos = 0;
    int name_len;
    int sock;
    struct sockaddr_in dest;
    int i;
    unsigned char rbuf[DNS_BUF];
    ssize_t nr;

    /* If already an IPv4 literal */
    if (inet_pton(AF_INET, host, ip) == 1) {
        return 0;
    }

    memset(query, 0, sizeof query);
    query[0] = 0x12;
    query[1] = 0x34;
    query[2] = 0x01;
    query[3] = 0x00;
    query[4] = 0x00;
    query[5] = 0x01;
    qpos = 12;
    name_len = encode_name(query + qpos, sizeof(query) - qpos, host);
    if (name_len == 0) {
        return -1;
    }
    qpos += (size_t)name_len;
    if (qpos + 4 > sizeof query) {
        return -1;
    }
    query[qpos++] = 0x00;
    query[qpos++] = 0x01; /* A */
    query[qpos++] = 0x00;
    query[qpos++] = 0x01; /* IN */

    sock = socket(AF_INET, SOCK_DGRAM, 0);
    if (sock < 0) {
        return -1;
    }
    memset(&dest, 0, sizeof dest);
    dest.sin_family = AF_INET;
    dest.sin_port = htons(53);
    inet_pton(AF_INET, "10.0.2.3", &dest.sin_addr);
    if (connect(sock, (struct sockaddr *)&dest, sizeof dest) < 0) {
        close(sock);
        return -1;
    }
    if (send(sock, query, qpos, 0) < 0) {
        close(sock);
        return -1;
    }
    /* Bound the DNS wait: nonblock + poll EAGAIN (blocking recv would wait forever). */
    (void)fcntl(sock, F_SETFL, O_NONBLOCK);
    for (i = 0; i < DATA_POLLS; i++) {
        nr = recv(sock, rbuf, sizeof rbuf, 0);
        if (nr < 0) {
            if (errno == EAGAIN || errno == EWOULDBLOCK) {
                continue;
            }
            break;
        }
        if (nr == 0) {
            continue;
        }
        if (parse_a_record(rbuf, (size_t)nr, ip) == 0) {
            close(sock);
            return 0;
        }
    }
    close(sock);
    (void)buf_has;
    (void)STATUS_POLLS;
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

int getaddrinfo(const char *node, const char *service,
    const struct addrinfo *hints, struct addrinfo **res) {
    struct addrinfo *ai;
    struct sockaddr_in *sa;
    unsigned char ip[4];
    unsigned short port = 0;
    int family = AF_INET;
    int socktype = SOCK_STREAM;
    int protocol = IPPROTO_TCP;

    if (res == NULL) {
        return EAI_SYSTEM;
    }
    *res = NULL;

    if (hints) {
        if (hints->ai_family != AF_UNSPEC && hints->ai_family != AF_INET) {
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

    if (node == NULL || node[0] == '\0') {
        if (hints && (hints->ai_flags & AI_PASSIVE)) {
            memset(ip, 0, 4);
        } else {
            ip[0] = 127;
            ip[1] = 0;
            ip[2] = 0;
            ip[3] = 1;
        }
    } else if (hints && (hints->ai_flags & AI_NUMERICHOST)) {
        if (inet_pton(AF_INET, node, ip) != 1) {
            return EAI_NONAME;
        }
    } else if (resolve_a(node, ip) < 0) {
        return EAI_NONAME;
    }

    ai = (struct addrinfo *)calloc(1, sizeof(*ai) + sizeof(*sa));
    if (ai == NULL) {
        return EAI_MEMORY;
    }
    sa = (struct sockaddr_in *)(ai + 1);
    memset(sa, 0, sizeof(*sa));
    sa->sin_family = AF_INET;
    sa->sin_port = htons(port);
    memcpy(&sa->sin_addr.s_addr, ip, 4);

    ai->ai_family = family;
    ai->ai_socktype = socktype;
    ai->ai_protocol = protocol;
    ai->ai_addrlen = sizeof(*sa);
    ai->ai_addr = (struct sockaddr *)sa;
    ai->ai_next = NULL;
    *res = ai;
    return 0;
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

int getnameinfo(const struct sockaddr *sa, socklen_t salen,
    char *host, socklen_t hostlen, char *serv, socklen_t servlen, int flags) {
    const struct sockaddr_in *in;
    (void)flags;
    if (sa == NULL || salen < sizeof(struct sockaddr_in) || sa->sa_family != AF_INET) {
        return EAI_FAMILY;
    }
    in = (const struct sockaddr_in *)sa;
    if (host && hostlen > 0) {
        if (inet_ntop(AF_INET, &in->sin_addr, host, hostlen) == NULL) {
            return EAI_OVERFLOW;
        }
    }
    if (serv && servlen > 0) {
        if (myos_u16_dec(serv, servlen, ntohs(in->sin_port)) < 0) {
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

    if (name == NULL || resolve_a(name, (unsigned char *)addr) < 0) {
        return NULL;
    }
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
