#ifndef _MYOS_NETINET_IN_H_
#define _MYOS_NETINET_IN_H_

#include <stdint.h>
#include <sys/socket.h>

#ifdef __cplusplus
extern "C" {
#endif

typedef uint32_t in_addr_t;
typedef uint16_t in_port_t;

struct in_addr {
    in_addr_t s_addr;
};

struct sockaddr_in {
    sa_family_t sin_family;
    in_port_t sin_port;
    struct in_addr sin_addr;
    char sin_zero[8];
};

struct in6_addr {
    uint8_t s6_addr[16];
};

#define IN6ADDR_ANY_INIT      { { 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0 } }
#define IN6ADDR_LOOPBACK_INIT { { 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1 } }

struct sockaddr_in6 {
    sa_family_t sin6_family;
    in_port_t sin6_port;
    uint32_t sin6_flowinfo;
    struct in6_addr sin6_addr;
    uint32_t sin6_scope_id;
};

#define INADDR_ANY       ((in_addr_t)0x00000000)
#define INADDR_BROADCAST ((in_addr_t)0xffffffff)
#define INADDR_LOOPBACK  ((in_addr_t)0x7f000001)
#define INADDR_NONE      ((in_addr_t)0xffffffff)

#define INET_ADDRSTRLEN  16
#define INET6_ADDRSTRLEN 46

/* Ports below this are the privileged ones (a convention; myos has no
 * privilege check). */
#define IPPORT_RESERVED 1024

#define IN6_IS_ADDR_UNSPECIFIED(a) \
    ((a)->s6_addr[0] == 0 && (a)->s6_addr[1] == 0 && (a)->s6_addr[2] == 0 \
     && (a)->s6_addr[3] == 0 && (a)->s6_addr[4] == 0 && (a)->s6_addr[5] == 0 \
     && (a)->s6_addr[6] == 0 && (a)->s6_addr[7] == 0 && (a)->s6_addr[8] == 0 \
     && (a)->s6_addr[9] == 0 && (a)->s6_addr[10] == 0 && (a)->s6_addr[11] == 0 \
     && (a)->s6_addr[12] == 0 && (a)->s6_addr[13] == 0 && (a)->s6_addr[14] == 0 \
     && (a)->s6_addr[15] == 0)
#define IN6_IS_ADDR_LOOPBACK(a) \
    ((a)->s6_addr[0] == 0 && (a)->s6_addr[1] == 0 && (a)->s6_addr[2] == 0 \
     && (a)->s6_addr[3] == 0 && (a)->s6_addr[4] == 0 && (a)->s6_addr[5] == 0 \
     && (a)->s6_addr[6] == 0 && (a)->s6_addr[7] == 0 && (a)->s6_addr[8] == 0 \
     && (a)->s6_addr[9] == 0 && (a)->s6_addr[10] == 0 && (a)->s6_addr[11] == 0 \
     && (a)->s6_addr[12] == 0 && (a)->s6_addr[13] == 0 && (a)->s6_addr[14] == 0 \
     && (a)->s6_addr[15] == 1)
#define IN6_IS_ADDR_V4MAPPED(a) \
    ((a)->s6_addr[0] == 0 && (a)->s6_addr[1] == 0 && (a)->s6_addr[2] == 0 \
     && (a)->s6_addr[3] == 0 && (a)->s6_addr[4] == 0 && (a)->s6_addr[5] == 0 \
     && (a)->s6_addr[6] == 0 && (a)->s6_addr[7] == 0 && (a)->s6_addr[8] == 0 \
     && (a)->s6_addr[9] == 0 && (a)->s6_addr[10] == 0xff && (a)->s6_addr[11] == 0xff)

#define IPPROTO_TCP 6
#define IPPROTO_UDP 17

/* TCP socket options (stubs in libgloss). */
#define TCP_NODELAY 1

static inline uint16_t htons(uint16_t x) {
#if __BYTE_ORDER__ == __ORDER_LITTLE_ENDIAN__
    return (uint16_t)((x << 8) | (x >> 8));
#else
    return x;
#endif
}
static inline uint16_t ntohs(uint16_t x) { return htons(x); }
static inline uint32_t htonl(uint32_t x) {
#if __BYTE_ORDER__ == __ORDER_LITTLE_ENDIAN__
    return ((x & 0xff000000u) >> 24) | ((x & 0x00ff0000u) >> 8)
         | ((x & 0x0000ff00u) << 8) | ((x & 0x000000ffu) << 24);
#else
    return x;
#endif
}
static inline uint32_t ntohl(uint32_t x) { return htonl(x); }

#ifdef __cplusplus
}
#endif

#endif /* _MYOS_NETINET_IN_H_ */
