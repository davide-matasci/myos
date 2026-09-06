/* Minimal stub for mbedtls x509 IP SAN parsing on freestanding myos. */
#ifndef MYOS_ARPA_INET_H
#define MYOS_ARPA_INET_H
#include <stdint.h>
#include <stddef.h>
static inline int inet_pton(int af, const char *src, void *dst) {
    (void)af; (void)src; (void)dst;
    return 0; /* fail: no IP SAN matching needed for example.com hostname */
}
static inline const char *inet_ntop(int af, const void *src, char *dst, size_t size) {
    (void)af; (void)src; (void)dst; (void)size;
    return NULL;
}
#ifndef AF_INET
#define AF_INET 2
#endif
#ifndef AF_INET6
#define AF_INET6 10
#endif
#endif
