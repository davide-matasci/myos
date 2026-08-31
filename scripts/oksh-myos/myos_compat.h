/* myos extras for oksh that newlib headers hide or omit. */
#ifndef _MYOS_OKSH_COMPAT_H_
#define _MYOS_OKSH_COMPAT_H_

#include <setjmp.h>
#include <sys/types.h>
#include <sys/myos_extra.h>
#include <time.h>

#ifndef sigjmp_buf
#define sigjmp_buf jmp_buf
#define sigsetjmp(buf, save) ((void)(save), setjmp(buf))
#define siglongjmp(buf, val) longjmp(buf, val)
#endif

#ifndef WCOREDUMP
#define WCOREDUMP(s) 0
#endif

#ifndef CLOCK_MONOTONIC
#define CLOCK_MONOTONIC ((clockid_t)4)
#endif
#ifndef CLOCK_REALTIME
#define CLOCK_REALTIME ((clockid_t)1)
#endif

int clock_gettime(clockid_t clock_id, struct timespec *tp);

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

#endif /* _MYOS_OKSH_COMPAT_H_ */
