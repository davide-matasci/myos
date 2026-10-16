/* What newlib + libgloss lack that openssl(1) uses (-include'd into every
 * compile, packages/openssl/build.sh; myos_stubs.c has the functions). */
#ifndef MYOS_OPENSSL_COMPAT_H
#define MYOS_OPENSSL_COMPAT_H

#include <netdb.h>
#include <sys/myos_extra.h>  /* lstat: newlib declares it on Cygwin and RTEMS only */

/* libgloss resolves names, not addresses (getnameinfo is numeric): s_server
 * and s_time name a peer by its address, as upstream does when the lookup
 * fails ("bad gethostbyaddr"). */
#define gethostbyaddr(addr, len, type) ((struct hostent *)0)

#endif
