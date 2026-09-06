/* Extra declarations for freestanding Vim on myos.
 * Compile-only via -include; not copied into the newlib sysroot. */
#ifndef _MYOS_VIM_COMPAT_H_
#define _MYOS_VIM_COMPAT_H_

#include <setjmp.h>
#include <sys/types.h>
#include <sys/myos_extra.h>
#include <time.h>
#include <unistd.h>
#include <errno.h>

#ifndef sigjmp_buf
#define sigjmp_buf jmp_buf
#define sigsetjmp(buf, save) ((void)(save), setjmp(buf))
#define siglongjmp(buf, val) longjmp(buf, val)
#endif

#ifndef MAXPATHLEN
#define MAXPATHLEN 1024
#endif

#ifndef PATH_MAX
#define PATH_MAX 1024
#endif

#ifndef WCOREDUMP
#define WCOREDUMP(s) 0
#endif

#ifndef CLOCK_REALTIME
#define CLOCK_REALTIME 0
#endif
#ifndef CLOCK_MONOTONIC
#define CLOCK_MONOTONIC 1
#endif
int clock_gettime(int clock_id, struct timespec *tp);

#ifndef TIOCGWINSZ
#define TIOCGWINSZ 0x5413
#endif

#endif /* _MYOS_VIM_COMPAT_H_ */
