#ifndef _MYOS_NET_IF_H_
#define _MYOS_NET_IF_H_

/* Network interfaces: netd's, as /net/ifaddrs lists them (ifaddrs.c). */

#include <sys/socket.h>

#ifdef __cplusplus
extern "C" {
#endif

#define IF_NAMESIZE 16
#define IFNAMSIZ    IF_NAMESIZE

/* Interface flags (struct ifaddrs's ifa_flags), Linux's values. */
#define IFF_UP        0x1
#define IFF_BROADCAST 0x2
#define IFF_LOOPBACK  0x8
#define IFF_RUNNING   0x40

struct if_nameindex {
    unsigned int if_index;
    char *if_name;
};

unsigned int if_nametoindex(const char *ifname);
char *if_indextoname(unsigned int ifindex, char *ifname);
struct if_nameindex *if_nameindex(void);
void if_freenameindex(struct if_nameindex *ptr);

#ifdef __cplusplus
}
#endif

#endif /* _MYOS_NET_IF_H_ */
