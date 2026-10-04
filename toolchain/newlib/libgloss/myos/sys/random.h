/* myos libgloss: getrandom (posix_extra.c), from /dev/urandom. Installed
 * into the newlib sysroot as <sys/random.h>. */
#ifndef _MYOS_SYS_RANDOM_H_
#define _MYOS_SYS_RANDOM_H_

#include <sys/types.h>

#ifdef __cplusplus
extern "C" {
#endif

/* Accepted and without effect: the kernel's generator never blocks. */
#define GRND_NONBLOCK 0x0001
#define GRND_RANDOM   0x0002
#define GRND_INSECURE 0x0004

ssize_t getrandom(void *buf, size_t buflen, unsigned int flags);

#ifdef __cplusplus
}
#endif

#endif /* _MYOS_SYS_RANDOM_H_ */
