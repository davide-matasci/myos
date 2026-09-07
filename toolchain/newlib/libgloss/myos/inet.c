#include <arpa/inet.h>
#include <errno.h>
#include <string.h>
#include "myos_fmt.h"

static int parse_ipv4(const char *src, unsigned char out[4]) {
    int i;
    const char *p = src;
    for (i = 0; i < 4; i++) {
        unsigned v = 0;
        int digits = 0;
        if (*p < '0' || *p > '9') {
            return 0;
        }
        while (*p >= '0' && *p <= '9') {
            v = v * 10u + (unsigned)(*p - '0');
            if (v > 255) {
                return 0;
            }
            p++;
            digits++;
            if (digits > 3) {
                return 0;
            }
        }
        out[i] = (unsigned char)v;
        if (i < 3) {
            if (*p != '.') {
                return 0;
            }
            p++;
        }
    }
    return *p == '\0';
}

static int fmt_ipv4(char *dst, size_t size, const unsigned char b[4]) {
    size_t pos = 0;
    int i;
    for (i = 0; i < 4; i++) {
        char tmp[4];
        int n = myos_u8_dec(tmp, sizeof tmp, b[i]);
        if (n < 0 || pos + (size_t)n + (i < 3 ? 1 : 0) >= size) {
            return -1;
        }
        memcpy(dst + pos, tmp, (size_t)n);
        pos += (size_t)n;
        if (i < 3) {
            dst[pos++] = '.';
        }
    }
    dst[pos] = '\0';
    return (int)pos;
}

in_addr_t inet_addr(const char *cp) {
    struct in_addr a;
    if (inet_aton(cp, &a) == 0) {
        return INADDR_NONE;
    }
    return a.s_addr;
}

int inet_aton(const char *cp, struct in_addr *inp) {
    unsigned char b[4];
    if (cp == NULL || inp == NULL || !parse_ipv4(cp, b)) {
        return 0;
    }
    memcpy(&inp->s_addr, b, 4);
    return 1;
}

char *inet_ntoa(struct in_addr in) {
    static char buf[INET_ADDRSTRLEN];
    unsigned char b[4];
    memcpy(b, &in.s_addr, 4);
    if (fmt_ipv4(buf, sizeof buf, b) < 0) {
        buf[0] = '\0';
    }
    return buf;
}

int inet_pton(int af, const char *src, void *dst) {
    unsigned char b[4];
    if (af != AF_INET) {
        errno = EAFNOSUPPORT;
        return -1;
    }
    if (src == NULL || dst == NULL || !parse_ipv4(src, b)) {
        return 0;
    }
    memcpy(dst, b, 4);
    return 1;
}

const char *inet_ntop(int af, const void *src, char *dst, size_t size) {
    if (af != AF_INET) {
        errno = EAFNOSUPPORT;
        return NULL;
    }
    if (src == NULL || dst == NULL || size < INET_ADDRSTRLEN) {
        errno = ENOSPC;
        return NULL;
    }
    if (fmt_ipv4(dst, size, (const unsigned char *)src) < 0) {
        errno = ENOSPC;
        return NULL;
    }
    return dst;
}
