/* What the X libraries expect from libc headers that libgloss's lack.
 * Compile-only (-include); not copied into the newlib sysroot. */
#ifndef _MYOS_X11_COMPAT_H_
#define _MYOS_X11_COMPAT_H_

#include <sys/ioctl.h>
#include <sys/utsname.h>  /* xtrans calls uname() without including it */

/* xtrans's BytesReadable asks ioctl for it; libgloss's ioctl() has no
 * FIONREAD and fails, which xtrans reports as an error. libX11 talks
 * through xcb, which never calls it. */
#ifndef FIONREAD
#define FIONREAD 0x541B
#endif

/* libxcb's xcb_util.c sizes a socket path with PATH_MAX without including
 * <limits.h>. */
#include <limits.h>
#ifndef PATH_MAX
#define PATH_MAX 1024
#endif

#endif
