/* myos_compat.h — what dropbear 2026.94 expects from libc headers that
 * newlib's lack. Force-included by build.sh (-include) so every TU gets
 * the types LTC and dropbear assume. */
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

/* newlib lacks setrlimit/getrlimit; dropbear only disables core dumps with
 * it (ports/dropbear/myos_shims.c). */
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

/* ports/dropbear/myos_shims.c: myos has no saved set-ids. */
int setresuid(uid_t ruid, uid_t euid, uid_t suid);
int setresgid(gid_t rgid, gid_t egid, gid_t sgid);

#endif /* DROPBEAR_MYOS_COMPAT_H_ */
