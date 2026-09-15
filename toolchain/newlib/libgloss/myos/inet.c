#include <arpa/inet.h>
#include <errno.h>
#include <string.h>
#include "myos_fmt.h"

static int parse_ipv4(const char *src, unsigned char out[4]) {
    /* BSD inet_addr semantics: 1-4 parts; each part may be decimal, 0x hex,
     * or 0-octal; a 1-part value is a 32-bit host-order number, 2-part =
     * a.b << 16 | c etc. os-test checks hex octets like "0xA.0XBC.345". */
    unsigned long parts[4];
    int nparts = 0;
    const char *p = src;
    if (p == NULL || *p == '\0') {
        return 0;
    }
    while (nparts < 4) {
        unsigned long v = 0;
        int digits = 0;
        int base = 10;
        if (*p == '0' && (p[1] == 'x' || p[1] == 'X')) {
            base = 16;
            p += 2;
            while ((*p >= '0' && *p <= '9') || (*p >= 'a' && *p <= 'f') ||
                   (*p >= 'A' && *p <= 'F')) {
                unsigned d = (unsigned)(*p - '0');
                if (*p >= 'a' && *p <= 'f') {
                    d = (unsigned)(*p - 'a') + 10u;
                } else if (*p >= 'A' && *p <= 'F') {
                    d = (unsigned)(*p - 'A') + 10u;
                }
                v = v * 16u + d;
                p++;
                digits++;
            }
        } else if (*p == '0') {
            base = 8;
            while (*p >= '0' && *p <= '7') {
                v = v * 8u + (unsigned)(*p - '0');
                p++;
                digits++;
            }
        } else if (*p >= '1' && *p <= '9') {
            while (*p >= '0' && *p <= '9') {
                v = v * 10u + (unsigned)(*p - '0');
                p++;
                digits++;
            }
        } else {
            return 0;
        }
        if (digits == 0 || v > 0xFFFFFFFFul) {
            return 0;
        }
        parts[nparts++] = v;
        if (*p == '.') {
            p++;
            if (nparts == 4 || *p == '\0') {
                return 0;
            }
            continue;
        }
        break;
    }
    if (*p != '\0' || nparts == 0) {
        return 0;
    }
    if (nparts == 4) {
        for (int i = 0; i < 4; i++) {
            if (parts[i] > 255) {
                return 0;
            }
            out[i] = (unsigned char)parts[i];
        }
    } else if (nparts == 3) {
        if (parts[0] > 255 || parts[1] > 255 || parts[2] > 0xFFFF) {
            return 0;
        }
        out[0] = (unsigned char)parts[0];
        out[1] = (unsigned char)parts[1];
        out[2] = (unsigned char)(parts[2] >> 8);
        out[3] = (unsigned char)(parts[2] & 0xFF);
    } else if (nparts == 2) {
        if (parts[0] > 255 || parts[1] > 0xFFFFFF) {
            return 0;
        }
        out[0] = (unsigned char)parts[0];
        out[1] = (unsigned char)(parts[1] >> 16);
        out[2] = (unsigned char)(parts[1] >> 8);
        out[3] = (unsigned char)(parts[1] & 0xFF);
    } else {
        out[0] = (unsigned char)(parts[0] >> 24);
        out[1] = (unsigned char)(parts[0] >> 16);
        out[2] = (unsigned char)(parts[0] >> 8);
        out[3] = (unsigned char)(parts[0] & 0xFF);
    }
    return 1;
}

static int parse_ipv4_strict(const char *src, unsigned char out[4]) {
    /* inet_pton: strict dotted-quad decimal per POSIX — no hex/octal, no
     * leading zeros (os-test requires "1.2.3.0000" rejected). */
    const char *p = src;
    for (int i = 0; i < 4; i++) {
        unsigned v = 0;
        int digits = 0;
        if (*p == '0' && p[1] >= '0' && p[1] <= '9') {
            return 0; /* leading zero */
        }
        while (*p >= '0' && *p <= '9') {
            v = v * 10u + (unsigned)(*p - '0');
            if (v > 255) {
                return 0;
            }
            p++;
            digits++;
        }
        if (digits == 0) {
            return 0;
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
    if (src == NULL || dst == NULL || !parse_ipv4_strict(src, b)) {
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
