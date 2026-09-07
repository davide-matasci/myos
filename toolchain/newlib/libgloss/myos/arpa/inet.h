#ifndef _MYOS_ARPA_INET_H_
#define _MYOS_ARPA_INET_H_

#include <netinet/in.h>
#include <stddef.h>

#ifdef __cplusplus
extern "C" {
#endif

in_addr_t inet_addr(const char *cp);
int inet_aton(const char *cp, struct in_addr *inp);
char *inet_ntoa(struct in_addr in);
int inet_pton(int af, const char *src, void *dst);
const char *inet_ntop(int af, const void *src, char *dst, size_t size);

#ifdef __cplusplus
}
#endif

#endif /* _MYOS_ARPA_INET_H_ */
