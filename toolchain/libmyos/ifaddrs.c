/*
 * Network interfaces: getifaddrs and the if_* name/index functions, from
 * netd's /net/ifaddrs, a line per address of each interface, the
 * interfaces in index order (lo is 1), each one's IPv4 line first:
 *
 *     lo 127.0.0.1 255.0.0.0 up,loopback
 *     lo ::1 ffff:ffff:ffff:ffff:ffff:ffff:ffff:ffff up,loopback
 *     net0 10.0.2.15 255.255.255.0 up,broadcast
 *     net0 fe80::5054:ff:fe12:3456 ffff:ffff:ffff:ffff:: up,broadcast
 *
 * the name, the address and netmask (IPv4 "-" while there is none) and the
 * flags. getifaddrs has an entry per line. Without netd the list is empty.
 */
#include <errno.h>
#include <fcntl.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>

#include <arpa/inet.h>
#include <ifaddrs.h>
#include <net/if.h>
#include <netinet/in.h>
#include "libmyos.h"
#include "libc.h"

#define IFADDRS_PATH "/net/ifaddrs"
#define IFADDRS_CAP 1024
/* Lines: lo's two, and three for each NIC (IPv4, link-local, SLAAC). */
#define MAX_IFS 24

struct ifline {
    char name[IF_NAMESIZE];
    unsigned int index; /* the interface's: 1 for the first name, ... */
    int family;         /* AF_INET, AF_INET6, or 0: no address */
    unsigned char addr[16];
    unsigned char mask[16];
    unsigned int flags;
};

/* The next whitespace-separated word of `*p` (NUL-terminated in place). */
static char *word(char **p) {
    char *s = *p;
    char *w;
    while (*s == ' ' || *s == '\t') {
        s++;
    }
    if (*s == '\0' || *s == '\n') {
        *p = s;
        return NULL;
    }
    w = s;
    while (*s != '\0' && *s != ' ' && *s != '\t' && *s != '\n') {
        s++;
    }
    if (*s == ' ' || *s == '\t') {
        *s++ = '\0';
    }
    *p = s;
    return w;
}

static unsigned int parse_flags(char *s) {
    unsigned int flags = 0;
    char *f = s;
    while (f != NULL && *f != '\0') {
        char *comma = strchr(f, ',');
        size_t n = comma != NULL ? (size_t)(comma - f) : strlen(f);
        if (n == 2 && memcmp(f, "up", 2) == 0) {
            flags |= IFF_UP | IFF_RUNNING;
        } else if (n == 8 && memcmp(f, "loopback", 8) == 0) {
            flags |= IFF_LOOPBACK;
        } else if (n == 9 && memcmp(f, "broadcast", 9) == 0) {
            flags |= IFF_BROADCAST;
        }
        f = comma != NULL ? comma + 1 : NULL;
    }
    return flags;
}

/* Read /net/ifaddrs into `ifs`; the number of interfaces. */
static int read_ifs(struct ifline *ifs) {
    char buf[IFADDRS_CAP];
    char *p = buf;
    ssize_t n;
    int count = 0;
    int fd = open(IFADDRS_PATH, O_RDONLY);
    if (fd < 0) {
        return 0;
    }
    n = read(fd, buf, sizeof buf - 1);
    close(fd);
    if (n <= 0) {
        return 0;
    }
    buf[n] = '\0';
    while (*p != '\0' && count < MAX_IFS) {
        char *line_end = strchr(p, '\n');
        char *name;
        char *addr;
        char *mask;
        char *flags;
        struct ifline *i = &ifs[count];
        if (line_end != NULL) {
            *line_end = '\0';
        }
        name = word(&p);
        addr = word(&p);
        mask = word(&p);
        flags = word(&p);
        if (name != NULL && strlen(name) < IF_NAMESIZE) {
            memset(i, 0, sizeof *i);
            strcpy(i->name, name);
            /* A name's lines are together: a new name is the next index. */
            i->index = count == 0 ? 1 : ifs[count - 1].index + (strcmp(ifs[count - 1].name, name) != 0);
            if (addr != NULL && mask != NULL && inet_pton(AF_INET, addr, i->addr) == 1
                && inet_pton(AF_INET, mask, i->mask) == 1) {
                i->family = AF_INET;
            } else if (addr != NULL && mask != NULL && inet_pton(AF_INET6, addr, i->addr) == 1
                && inet_pton(AF_INET6, mask, i->mask) == 1) {
                i->family = AF_INET6;
            }
            i->flags = flags != NULL ? parse_flags(flags) : 0;
            count++;
        }
        if (line_end == NULL) {
            break;
        }
        p = line_end + 1;
    }
    return count;
}

/* One allocation per entry: the entry, its name and its addresses. */
struct ifaddrs_block {
    struct ifaddrs ifa;
    char name[IF_NAMESIZE];
    union {
        struct sockaddr_in in;
        struct sockaddr_in6 in6;
    } addr, mask, broad;
};

static void set_in(struct sockaddr_in *sin, const unsigned char *a) {
    memset(sin, 0, sizeof *sin);
    sin->sin_family = AF_INET;
    memcpy(&sin->sin_addr, a, 4);
}

/* A link-local address names its interface (sin6_scope_id). */
static void set_in6(struct sockaddr_in6 *sin6, const unsigned char *a, unsigned int index) {
    memset(sin6, 0, sizeof *sin6);
    sin6->sin6_family = AF_INET6;
    memcpy(sin6->sin6_addr.s6_addr, a, 16);
    if (a[0] == 0xfe && (a[1] & 0xc0) == 0x80) {
        sin6->sin6_scope_id = index;
    }
}

/* The interfaces: each one's name, by index (1 and up); how many. */
static int if_names(const struct ifline *ifs, int n, const char **out) {
    int k;
    int count = 0;
    for (k = 0; k < n; k++) {
        if (ifs[k].index > (unsigned int)count) {
            out[count++] = ifs[k].name;
        }
    }
    return count;
}

int __myos_getifaddrs(struct ifaddrs **ifap) {
    struct ifline ifs[MAX_IFS];
    struct ifaddrs *head = NULL;
    struct ifaddrs **tail = &head;
    int n;
    int k;
    if (ifap == NULL) {
        errno = EINVAL;
        return -1;
    }
    n = read_ifs(ifs);
    for (k = 0; k < n; k++) {
        struct ifaddrs_block *b = calloc(1, sizeof *b);
        if (b == NULL) {
            __myos_freeifaddrs(head);
            errno = ENOMEM;
            return -1;
        }
        strcpy(b->name, ifs[k].name);
        b->ifa.ifa_name = b->name;
        b->ifa.ifa_flags = ifs[k].flags;
        if (ifs[k].family == AF_INET) {
            unsigned char broad[4];
            int j;
            set_in(&b->addr.in, ifs[k].addr);
            set_in(&b->mask.in, ifs[k].mask);
            b->ifa.ifa_addr = (struct sockaddr *)&b->addr;
            b->ifa.ifa_netmask = (struct sockaddr *)&b->mask;
            if (ifs[k].flags & IFF_BROADCAST) {
                for (j = 0; j < 4; j++) {
                    broad[j] = ifs[k].addr[j] | (unsigned char)~ifs[k].mask[j];
                }
                set_in(&b->broad.in, broad);
                b->ifa.ifa_broadaddr = (struct sockaddr *)&b->broad;
            }
        } else if (ifs[k].family == AF_INET6) {
            set_in6(&b->addr.in6, ifs[k].addr, ifs[k].index);
            set_in6(&b->mask.in6, ifs[k].mask, 0);
            b->ifa.ifa_addr = (struct sockaddr *)&b->addr;
            b->ifa.ifa_netmask = (struct sockaddr *)&b->mask;
            /* IPv6 has no broadcast address. */
            b->ifa.ifa_flags &= ~(unsigned int)IFF_BROADCAST;
        }
        *tail = &b->ifa;
        tail = &b->ifa.ifa_next;
    }
    *ifap = head;
    return 0;
}

void __myos_freeifaddrs(struct ifaddrs *ifa) {
    while (ifa != NULL) {
        struct ifaddrs *next = ifa->ifa_next;
        free(ifa); /* the start of its ifaddrs_block */
        ifa = next;
    }
}

unsigned int __myos_if_nametoindex(const char *ifname) {
    struct ifline ifs[MAX_IFS];
    const char *name[MAX_IFS];
    int n = if_names(ifs, read_ifs(ifs), name);
    int k;
    for (k = 0; k < n; k++) {
        if (strcmp(name[k], ifname) == 0) {
            return (unsigned int)k + 1;
        }
    }
    errno = ENXIO;
    return 0;
}

char *__myos_if_indextoname(unsigned int ifindex, char *ifname) {
    struct ifline ifs[MAX_IFS];
    const char *name[MAX_IFS];
    int n = if_names(ifs, read_ifs(ifs), name);
    if (ifindex == 0 || ifindex > (unsigned int)n) {
        errno = ENXIO;
        return NULL;
    }
    strcpy(ifname, name[ifindex - 1]);
    return ifname;
}

struct if_nameindex *__myos_if_nameindex(void) {
    struct ifline ifs[MAX_IFS];
    const char *name[MAX_IFS];
    int n = if_names(ifs, read_ifs(ifs), name);
    int k;
    /* The array, its zero terminator, then the names. */
    struct if_nameindex *list = malloc((size_t)(n + 1) * sizeof *list
        + (size_t)n * IF_NAMESIZE);
    char *names;
    if (list == NULL) {
        errno = ENOBUFS;
        return NULL;
    }
    names = (char *)(list + n + 1);
    for (k = 0; k < n; k++) {
        strcpy(names + k * IF_NAMESIZE, name[k]);
        list[k].if_index = (unsigned int)k + 1;
        list[k].if_name = names + k * IF_NAMESIZE;
    }
    list[n].if_index = 0;
    list[n].if_name = NULL;
    return list;
}

void __myos_if_freenameindex(struct if_nameindex *ptr) {
    free(ptr);
}

MYOS_PUBLIC(getifaddrs)
MYOS_PUBLIC(freeifaddrs)
MYOS_ALIAS(if_nametoindex)
MYOS_ALIAS(if_indextoname)
MYOS_ALIAS(if_nameindex)
MYOS_ALIAS(if_freenameindex)
