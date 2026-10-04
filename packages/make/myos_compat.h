/* Extra declarations newlib headers omit. Compile-only for make; not
 * copied into the newlib sysroot. */
#ifndef _MYOS_MAKE_COMPAT_H_
#define _MYOS_MAKE_COMPAT_H_

#include <sys/types.h>
#include <sys/stat.h>
#include <sys/wait.h>
#include <signal.h>
#include <unistd.h>
#include <sys/myos_extra.h>  /* lstat: newlib declares it on Cygwin and RTEMS only */

/* newlib's unistd.h does not advertise POSIX compliance without a feature
 * macro; makeint.h only omits its own (conflicting) getcwd/lseek prototypes
 * on POSIX systems. */
#ifndef _POSIX_VERSION
#define _POSIX_VERSION 199506L
#endif

#ifndef W_EXITCODE
#define W_EXITCODE(ret, sig) ((ret) << 8 | (sig))
#endif

/* NLS is off, but main.c calls bindtextdomain()/textdomain() anyway; the
 * link resolves them somewhere weak-ish and the first call faults on the
 * guest. Compile them out entirely. */
#define bindtextdomain(domain, dir) ((void)0)
#define textdomain(domain) ((void)0)

#endif /* _MYOS_MAKE_COMPAT_H_ */
