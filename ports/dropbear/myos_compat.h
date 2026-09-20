/* myos_compat.h — myos shims/gaps for dropbear 2026.94.
 * Force-included by build.sh (-include) so every TU gets the types LTC and
 * dropbear assume. Struct passwd/group come from newlib's <pwd.h>/<grp.h> —
 * only the lookup functions are missing (see myos_shims.c). */
#ifndef DROPBEAR_MYOS_COMPAT_H_
#define DROPBEAR_MYOS_COMPAT_H_

#include <stddef.h>
#include <time.h>
#include <errno.h>
#include <string.h>
#include <unistd.h>
#include <fcntl.h>
#include <stdlib.h>
#include <pwd.h>
#include <grp.h>

/* newlib termios lacks these flags (dbclient raw-mode + termcodes use them). */
#ifndef IXANY
#define IXANY 0
#endif
#ifndef VSTOP
#define VSTOP 0013
#endif
#ifndef VSTART
#define VSTART 0011
#endif
#ifndef VEOL
#define VEOL 0
#endif
#ifndef PARENB
#define PARENB 0000400
#endif
#ifndef PARODD
#define PARODD 0001000
#endif
#ifndef CS7
#define CS7 0000040
#endif

/* newlib netdb lacks IPPORT_RESERVED (dropbear uses it for privileged ports). */
#ifndef IPPORT_RESERVED
#define IPPORT_RESERVED 1024
#endif

/* newlib lacks setrlimit/getrlimit; dropbear only disables core dumps with it. */
#ifndef RLIMIT_CORE
#define RLIMIT_CORE 4
#endif
#ifndef RLIM_INFINITY
#define RLIM_INFINITY ((unsigned long)-1)
#endif
struct rlimit {
	unsigned long rlim_cur;
	unsigned long rlim_max;
};
int getrlimit(int resource, struct rlimit *rlim);
int setrlimit(int resource, const struct rlimit *rlim);

/* myos has no nanosleep; dropbear's netio backoff uses it. */
struct timespec;
int nanosleep(const struct timespec *req, struct timespec *rem);

/* provided by ports/dropbear/myos_shims.c (myos is single-user). */
int setresuid(uid_t ruid, uid_t euid, uid_t suid);
int setresgid(gid_t rgid, gid_t egid, gid_t sgid);

#endif /* DROPBEAR_MYOS_COMPAT_H_ */
