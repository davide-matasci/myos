/* What TinyX expects from libc headers that newlib's leave out for myos.
 * Compile-only (-include); not copied into the newlib sysroot. */
#ifndef _MYOS_TINYX_COMPAT_H_
#define _MYOS_TINYX_COMPAT_H_

#include <sys/types.h>
#include <sys/stat.h>
#include <sys/myos_extra.h>   /* lstat: newlib declares it on Cygwin and RTEMS only */

/* xtrans's TCP listener looks the port up by service name first; libgloss
 * has no services database, so the lookup finds nothing and xtrans uses
 * the number (6000 + display). The server listens on TCP only when asked
 * (-listen tcp). */
struct servent {
    char *s_name;
    char **s_aliases;
    int s_port;
    char *s_proto;
};

static inline struct servent *getservbyname(const char *name, const char *proto) {
    (void)name;
    (void)proto;
    return 0;
}

#endif
