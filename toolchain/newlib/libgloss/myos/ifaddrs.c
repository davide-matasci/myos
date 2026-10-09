/*
 * Network interfaces: getifaddrs and the if_* name/index functions, from
 * netd's /net/ifaddrs, a line per interface in index order (lo is 1):
 *
 *     lo 127.0.0.1 255.0.0.0 up,loopback
 *     net0 10.0.2.15 255.255.255.0 up,broadcast
 *
 * the name, the IPv4 address and netmask ("-" while there is none) and the
 * flags. Without netd the list is empty.
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

#define IFADDRS_PATH "/net/ifaddrs"
#define IFADDRS_CAP 512
#define MAX_IFS 8

struct ifline {
    char name[IF_NAMESIZE];
    int has_addr;
    struct in_addr addr;
    struct in_addr mask;
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
            i->has_addr = addr != NULL && mask != NULL
                && inet_pton(AF_INET, addr, &i->addr) == 1
                && inet_pton(AF_INET, mask, &i->mask) == 1;
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

/* One allocation per interface: the entry, its name and its addresses. */
struct ifaddrs_block {
    struct ifaddrs ifa;
    char name[IF_NAMESIZE];
    struct sockaddr_in addr;
    struct sockaddr_in mask;
    struct sockaddr_in broad;
};

static void set_in(struct sockaddr_in *sin, struct in_addr a) {
    memset(sin, 0, sizeof *sin);
    sin->sin_family = AF_INET;
    sin->sin_addr = a;
}

int getifaddrs(struct ifaddrs **ifap) {
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
            freeifaddrs(head);
            errno = ENOMEM;
            return -1;
        }
        strcpy(b->name, ifs[k].name);
        b->ifa.ifa_name = b->name;
        b->ifa.ifa_flags = ifs[k].flags;
        if (ifs[k].has_addr) {
            struct in_addr broad;
            set_in(&b->addr, ifs[k].addr);
            set_in(&b->mask, ifs[k].mask);
            b->ifa.ifa_addr = (struct sockaddr *)&b->addr;
            b->ifa.ifa_netmask = (struct sockaddr *)&b->mask;
            if (ifs[k].flags & IFF_BROADCAST) {
                broad.s_addr = ifs[k].addr.s_addr | ~ifs[k].mask.s_addr;
                set_in(&b->broad, broad);
                b->ifa.ifa_broadaddr = (struct sockaddr *)&b->broad;
            }
        }
        *tail = &b->ifa;
        tail = &b->ifa.ifa_next;
    }
    *ifap = head;
    return 0;
}

void freeifaddrs(struct ifaddrs *ifa) {
    while (ifa != NULL) {
        struct ifaddrs *next = ifa->ifa_next;
        free(ifa); /* the start of its ifaddrs_block */
        ifa = next;
    }
}

unsigned int if_nametoindex(const char *ifname) {
    struct ifline ifs[MAX_IFS];
    int n = read_ifs(ifs);
    int k;
    for (k = 0; k < n; k++) {
        if (strcmp(ifs[k].name, ifname) == 0) {
            return (unsigned int)k + 1;
        }
    }
    errno = ENXIO;
    return 0;
}

char *if_indextoname(unsigned int ifindex, char *ifname) {
    struct ifline ifs[MAX_IFS];
    int n = read_ifs(ifs);
    if (ifindex == 0 || ifindex > (unsigned int)n) {
        errno = ENXIO;
        return NULL;
    }
    strcpy(ifname, ifs[ifindex - 1].name);
    return ifname;
}

struct if_nameindex *if_nameindex(void) {
    struct ifline ifs[MAX_IFS];
    int n = read_ifs(ifs);
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
        strcpy(names + k * IF_NAMESIZE, ifs[k].name);
        list[k].if_index = (unsigned int)k + 1;
        list[k].if_name = names + k * IF_NAMESIZE;
    }
    list[n].if_index = 0;
    list[n].if_name = NULL;
    return list;
}

void if_freenameindex(struct if_nameindex *ptr) {
    free(ptr);
}
