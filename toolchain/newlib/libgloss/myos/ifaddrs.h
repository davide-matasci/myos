#ifndef _IFADDRS_H_
#define _IFADDRS_H_

#include <sys/types.h>
#include <sys/socket.h>

#ifdef __cplusplus
extern "C" {
#endif

struct ifaddrs {
    struct ifaddrs  *ifa_next;
    char            *ifa_name;
    unsigned int     ifa_flags;
    struct sockaddr *ifa_addr;
    struct sockaddr *ifa_netmask;
    union {
        struct sockaddr *ifu_broadaddr;
        struct sockaddr *ifu_dstaddr;
    } ifa_ifu;
    void            *ifa_data;
};

#ifndef ifa_broadaddr
#define ifa_broadaddr ifa_ifu.ifu_broadaddr
#endif
#ifndef ifa_dstaddr
#define ifa_dstaddr   ifa_ifu.ifu_dstaddr
#endif

int getifaddrs(struct ifaddrs **ifap);
void freeifaddrs(struct ifaddrs *ifa);

#ifdef __cplusplus
}
#endif

#endif /* _IFADDRS_H_ */
