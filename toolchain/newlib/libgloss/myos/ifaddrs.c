/* getifaddrs — report loopback + a single LAN-ish interface for os-test/udp.
 * Real NIC enumeration can replace this later; values match netd's single IF. */
#include <errno.h>
#include <ifaddrs.h>
#include <net/if.h>
#include <netinet/in.h>
#include <stdlib.h>
#include <string.h>

#include <arpa/inet.h>

struct myos_ifa_blob {
    struct ifaddrs ifa;
    char name[16];
    struct sockaddr_in addr;
    struct sockaddr_in mask;
};

int getifaddrs(struct ifaddrs **ifap) {
    struct myos_ifa_blob *lo;
    struct myos_ifa_blob *eth;
    if (ifap == NULL) {
        errno = EINVAL;
        return -1;
    }
    lo = (struct myos_ifa_blob *)calloc(1, sizeof(*lo));
    eth = (struct myos_ifa_blob *)calloc(1, sizeof(*eth));
    if (lo == NULL || eth == NULL) {
        free(lo);
        free(eth);
        errno = ENOMEM;
        return -1;
    }

    memcpy(lo->name, "lo", 3);
    lo->addr.sin_family = AF_INET;
    lo->addr.sin_addr.s_addr = htonl(INADDR_LOOPBACK);
    lo->mask.sin_family = AF_INET;
    lo->mask.sin_addr.s_addr = htonl(0xff000000u);
    lo->ifa.ifa_next = &eth->ifa;
    lo->ifa.ifa_name = lo->name;
    lo->ifa.ifa_flags = IFF_UP | IFF_LOOPBACK | IFF_RUNNING;
    lo->ifa.ifa_addr = (struct sockaddr *)&lo->addr;
    lo->ifa.ifa_netmask = (struct sockaddr *)&lo->mask;

    memcpy(eth->name, "eth0", 5);
    eth->addr.sin_family = AF_INET;
    /* 10.0.2.15 — qemu user-mode / slirp default guest address. */
    eth->addr.sin_addr.s_addr = htonl(0x0a00020fu);
    eth->mask.sin_family = AF_INET;
    eth->mask.sin_addr.s_addr = htonl(0xffffff00u);
    eth->ifa.ifa_next = NULL;
    eth->ifa.ifa_name = eth->name;
    eth->ifa.ifa_flags = IFF_UP | IFF_BROADCAST | IFF_RUNNING | IFF_MULTICAST;
    eth->ifa.ifa_addr = (struct sockaddr *)&eth->addr;
    eth->ifa.ifa_netmask = (struct sockaddr *)&eth->mask;

    *ifap = &lo->ifa;
    return 0;
}

void freeifaddrs(struct ifaddrs *ifa) {
    while (ifa) {
        struct ifaddrs *next = ifa->ifa_next;
        free(ifa);
        ifa = next;
    }
}

unsigned int if_nametoindex(const char *ifname) {
    if (ifname == NULL) {
        return 0;
    }
    if (strcmp(ifname, "lo") == 0) {
        return 1;
    }
    if (strcmp(ifname, "eth0") == 0) {
        return 2;
    }
    return 0;
}

char *if_indextoname(unsigned int ifindex, char *ifname) {
    if (ifname == NULL) {
        return NULL;
    }
    if (ifindex == 1) {
        strcpy(ifname, "lo");
        return ifname;
    }
    if (ifindex == 2) {
        strcpy(ifname, "eth0");
        return ifname;
    }
    return NULL;
}

struct if_nameindex *if_nameindex(void) {
    struct if_nameindex *list = calloc(3, sizeof(*list));
    if (list == NULL) {
        return NULL;
    }
    list[0].if_index = 1;
    list[0].if_name = strdup("lo");
    list[1].if_index = 2;
    list[1].if_name = strdup("eth0");
    list[2].if_index = 0;
    list[2].if_name = NULL;
    if (list[0].if_name == NULL || list[1].if_name == NULL) {
        free(list[0].if_name);
        free(list[1].if_name);
        free(list);
        return NULL;
    }
    return list;
}

void if_freenameindex(struct if_nameindex *ptr) {
    if (ptr == NULL) {
        return;
    }
    for (struct if_nameindex *p = ptr; p->if_name != NULL; p++) {
        free(p->if_name);
    }
    free(ptr);
}
