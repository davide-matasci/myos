/* Extra POSIX/BSD declarations newlib headers hide or omit. Compile-only
 * for oksh; not copied into the newlib sysroot. */
#ifndef _MYOS_OKSH_COMPAT_H_
#define _MYOS_OKSH_COMPAT_H_

#include <setjmp.h>
#include <sys/types.h>
#include <sys/myos_extra.h>
#include <unistd.h>

#ifndef sigjmp_buf
#define sigjmp_buf jmp_buf
#define sigsetjmp(buf, save) ((void)(save), setjmp(buf))
#define siglongjmp(buf, val) longjmp(buf, val)
#endif

#ifndef WCOREDUMP
#define WCOREDUMP(s) 0
#endif

#ifndef MAXPATHLEN
#define MAXPATHLEN 1024
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

/*
 * jobs.c still compiles FMONITOR=0 paths that mention these. Do not add
 * libgloss stubs; skip at compile time. Do not #define killpg: newlib's
 * sys/signal.h already declares it, and the macro expanded inside that
 * prototype. jobs/c_ksh patches drop killpg calls instead.
 */
#define tcgetattr(fd, t) ((void)(fd), (void)(t), -1)
#define tcsetattr(fd, a, t) ((void)(fd), (void)(a), (void)(t), 0)
#define tcgetpgrp(fd) ((void)(fd), (pid_t)-1)
#define tcsetpgrp(fd, p) ((void)(fd), (void)(p), -1)
#define setpgid(p, g) ((void)(p), (void)(g), 0)

#endif /* _MYOS_OKSH_COMPAT_H_ */
