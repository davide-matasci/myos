/* What the X libraries expect from libc headers that libgloss's lack.
 * Compile-only (-include); not copied into the newlib sysroot. */
#ifndef _MYOS_X11_COMPAT_H_
#define _MYOS_X11_COMPAT_H_

#include <limits.h>
#include <netinet/in.h>
#include <sys/ioctl.h>
#include <sys/utsname.h>  /* xtrans calls uname() without including it */

/* xtrans's BytesReadable asks ioctl for it; libgloss's ioctl() has no
 * FIONREAD and fails, which xtrans reports as an error. libX11 talks
 * through xcb, which never calls it. */
#ifndef FIONREAD
#define FIONREAD 0x541B
#endif

#ifndef PATH_MAX
#define PATH_MAX 1024
#endif

#ifndef IN6_IS_ADDR_LOOPBACK
#define IN6_IS_ADDR_LOOPBACK(a) \
    (((const uint32_t *)(a)->s6_addr)[0] == 0 && ((const uint32_t *)(a)->s6_addr)[1] == 0 \
     && ((const uint32_t *)(a)->s6_addr)[2] == 0 && (a)->s6_addr[12] == 0 && (a)->s6_addr[13] == 0 \
     && (a)->s6_addr[14] == 0 && (a)->s6_addr[15] == 1)
#endif
#ifndef IN6_IS_ADDR_V4MAPPED
#define IN6_IS_ADDR_V4MAPPED(a) \
    (((const uint32_t *)(a)->s6_addr)[0] == 0 && ((const uint32_t *)(a)->s6_addr)[1] == 0 \
     && (a)->s6_addr[8] == 0 && (a)->s6_addr[9] == 0 && (a)->s6_addr[10] == 0xff \
     && (a)->s6_addr[11] == 0xff)
#endif

#endif
