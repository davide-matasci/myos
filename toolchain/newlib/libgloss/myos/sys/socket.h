#ifndef _MYOS_SYS_SOCKET_H_
#define _MYOS_SYS_SOCKET_H_

#include <sys/types.h>

typedef unsigned int socklen_t;

#define AF_INET 2
#define SOCK_DGRAM 2
#define IPPROTO_UDP 17

struct sockaddr {
    unsigned short sa_family;
    char sa_data[14];
};

struct sockaddr_in {
    unsigned short sin_family;
    unsigned short sin_port;
    unsigned int sin_addr;
    char sin_zero[8];
};

static inline int socket(int domain, int type, int protocol) {
    (void)domain;
    (void)type;
    (void)protocol;
    return -1;
}

static inline int bind(int sockfd, const struct sockaddr *addr, socklen_t addrlen) {
    (void)sockfd;
    (void)addr;
    (void)addrlen;
    return -1;
}

static inline ssize_t recvfrom(int sockfd, void *buf, size_t len, int flags,
    struct sockaddr *src_addr, socklen_t *addrlen) {
    (void)sockfd;
    (void)buf;
    (void)len;
    (void)flags;
    (void)src_addr;
    (void)addrlen;
    return -1;
}

static inline ssize_t sendto(int sockfd, const void *buf, size_t len, int flags,
    const struct sockaddr *dest_addr, socklen_t addrlen) {
    (void)sockfd;
    (void)buf;
    (void)len;
    (void)flags;
    (void)dest_addr;
    (void)addrlen;
    return -1;
}

static inline int close(int fd) {
    (void)fd;
    return -1;
}

#endif /* _MYOS_SYS_SOCKET_H_ */
