/* Extra POSIX/BSD declarations newlib headers omit. Compile-only for
 * oksh; not copied into the newlib sysroot. */
#ifndef _MYOS_OKSH_COMPAT_H_
#define _MYOS_OKSH_COMPAT_H_

#include <sys/types.h>
#include <sys/myos_extra.h>
#include <sys/wait.h>

/* No core dumps on myos; newlib's sys/wait.h has no WCOREDUMP. */
#ifndef WCOREDUMP
#define WCOREDUMP(s) 0
#endif

#ifndef MAXPATHLEN
#define MAXPATHLEN 1024
#endif

#endif /* _MYOS_OKSH_COMPAT_H_ */
