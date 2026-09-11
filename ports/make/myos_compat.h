/* Extra POSIX/BSD declarations newlib headers hide or omit. Compile-only
 * for make; not copied into the newlib sysroot. */
#ifndef _MYOS_MAKE_COMPAT_H_
#define _MYOS_MAKE_COMPAT_H_

#include <sys/types.h>
#include <sys/stat.h>
#include <signal.h>
#include <unistd.h>
#include <sys/myos_extra.h>  /* lstat, execve, waitpid protos */

/* newlib's unistd.h does not advertise POSIX compliance; makeint.h only
 * omits its own (conflicting) getcwd/lseek prototypes on POSIX systems. */
#ifndef _POSIX_VERSION
#define _POSIX_VERSION 199506L
#endif

#ifndef S_ISCHR
#define S_ISCHR(m) (((m) & 0170000) == 0020000)
#endif
#ifndef S_ISDIR
#define S_ISDIR(m) (((m) & 0170000) == 0040000)
#endif
#ifndef S_ISREG
#define S_ISREG(m) (((m) & 0170000) == 0100000)
#endif
#ifndef S_ISLNK
#define S_ISLNK(m) (((m) & 0170000) == 0120000)
#endif
#ifndef S_ISFIFO
#define S_ISFIFO(m) (((m) & 0170000) == 0010000)
#endif
#ifndef S_ISBLK
#define S_ISBLK(m) (((m) & 0170000) == 0060000)
#endif
#ifndef S_ISSOCK
#define S_ISSOCK(m) (((m) & 0170000) == 0140000)
#endif

#ifndef MAXPATHLEN
#define MAXPATHLEN 1024
#endif

#ifndef W_EXITCODE
#define W_EXITCODE(ret, sig) ((ret) << 8 | (sig))
#endif

/* newlib lacks these; make uses them only when available. */
#ifndef SA_RESTART
#define SA_RESTART 0
#endif

/* newlib dirent.h has no d_type constants; glob.c uses them when
 * HAVE_STRUCT_DIRENT_D_TYPE is set. Values follow standard Linux. */
#ifndef DT_UNKNOWN
#define DT_UNKNOWN 0
#define DT_FIFO 1
#define DT_CHR 2
#define DT_DIR 4
#define DT_BLK 6
#define DT_REG 8
#define DT_LNK 10
#define DT_SOCK 12
#endif

/* NLS is off, but main.c calls bindtextdomain()/textdomain() anyway; the
 * link resolves them somewhere weak-ish and the first call faults on the
 * guest. Compile them out entirely. */
#define bindtextdomain(domain, dir) ((void)0)
#define textdomain(domain) ((void)0)

#endif /* _MYOS_MAKE_COMPAT_H_ */
