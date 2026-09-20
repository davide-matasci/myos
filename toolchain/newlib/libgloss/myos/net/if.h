#ifndef _NET_IF_H_
#define _NET_IF_H_

#include <sys/types.h>

#define IFF_UP          0x1
#define IFF_BROADCAST   0x2
#define IFF_DEBUG       0x4
#define IFF_LOOPBACK    0x8
#define IFF_POINTOPOINT 0x10
#define IFF_RUNNING     0x40
#define IFF_NOARP       0x80
#define IFF_PROMISC     0x100
#define IFF_MULTICAST   0x800

#define IF_NAMESIZE 16

struct if_nameindex {
    unsigned int if_index;
    char *if_name;
};

unsigned int if_nametoindex(const char *ifname);
char *if_indextoname(unsigned int ifindex, char *ifname);
struct if_nameindex *if_nameindex(void);
void if_freenameindex(struct if_nameindex *ptr);

#endif /* _NET_IF_H_ */
