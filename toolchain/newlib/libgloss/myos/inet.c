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
     * leading zeros (os-test requires "1.2.3.0000" rejected). Also used for
     * the IPv4 tail of IPv4-mapped IPv6 addresses. */
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

/* RFC 4291 / POSIX inet_pton(AF_INET6). Based on Paul Vixie / ISC, with:
 * - max 4 hex digits per group (os-test rejects "04567")
 * - IPv4-mapped tail via parse_ipv4_strict (reject leading zeros)
 * Returns 1 on success, 0 on invalid presentation. */
static int parse_ipv6(const char *src, unsigned char out[16]) {
    unsigned char tmp[16];
    unsigned char *tp;
    unsigned char *endp;
    unsigned char *colonp;
    const char *curtok;
    int ch;
    int saw_xdigit;
    int digits;
    unsigned val;

    if (src == NULL) {
        return 0;
    }
    for (int i = 0; i < 16; i++) {
        tmp[i] = 0;
    }
    tp = tmp;
    endp = tmp + 16;
    colonp = NULL;
    if (*src == ':') {
        if (*++src != ':') {
            return 0;
        }
    }
    curtok = src;
    saw_xdigit = 0;
    digits = 0;
    val = 0;
    while ((ch = (unsigned char)*src++) != '\0') {
        int d = -1;
        if (ch >= '0' && ch <= '9') {
            d = ch - '0';
        } else if (ch >= 'a' && ch <= 'f') {
            d = ch - 'a' + 10;
        } else if (ch >= 'A' && ch <= 'F') {
            d = ch - 'A' + 10;
        }
        if (d >= 0) {
            if (++digits > 4) {
                return 0;
            }
            val = (val << 4) | (unsigned)d;
            if (val > 0xffff) {
                return 0;
            }
            saw_xdigit = 1;
            continue;
        }
        if (ch == ':') {
            curtok = src;
            if (!saw_xdigit) {
                if (colonp) {
                    return 0;
                }
                colonp = tp;
                continue;
            }
            if (tp + 2 > endp) {
                return 0;
            }
            *tp++ = (unsigned char)(val >> 8);
            *tp++ = (unsigned char)(val & 0xff);
            saw_xdigit = 0;
            digits = 0;
            val = 0;
            continue;
        }
        if (ch == '.' && (tp + 4) <= endp && parse_ipv4_strict(curtok, tp)) {
            tp += 4;
            saw_xdigit = 0;
            digits = 0;
            break;
        }
        return 0;
    }
    if (saw_xdigit) {
        if (tp + 2 > endp) {
            return 0;
        }
        *tp++ = (unsigned char)(val >> 8);
        *tp++ = (unsigned char)(val & 0xff);
    }
    if (colonp != NULL) {
        int n = (int)(tp - colonp);
        int i;
        for (i = 1; i <= n; i++) {
            endp[-i] = colonp[n - i];
            colonp[n - i] = 0;
        }
        tp = endp;
    }
    if (tp != endp) {
        return 0;
    }
    for (int i = 0; i < 16; i++) {
        out[i] = tmp[i];
    }
    return 1;
}

int inet_pton(int af, const char *src, void *dst) {
    unsigned char b4[4];
    unsigned char b16[16];
    if (af == AF_INET) {
        if (src == NULL || dst == NULL || !parse_ipv4_strict(src, b4)) {
            return 0;
        }
        memcpy(dst, b4, 4);
        return 1;
    }
    if (af == AF_INET6) {
        if (src == NULL || dst == NULL || !parse_ipv6(src, b16)) {
            return 0;
        }
        memcpy(dst, b16, 16);
        return 1;
    }
    errno = EAFNOSUPPORT;
    return -1;
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
