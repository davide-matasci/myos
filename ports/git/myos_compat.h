/* Extra declarations for freestanding Git on myos.
 * Compile-only via -include; not copied into the newlib sysroot. */
#ifndef _MYOS_GIT_COMPAT_H_
#define _MYOS_GIT_COMPAT_H_

#include <setjmp.h>
#include <sys/types.h>
#include <sys/myos_extra.h>
#include <time.h>
#include <unistd.h>
#include <errno.h>
#include <stdio.h>

#include <limits.h>

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

#ifndef NAME_MAX
#define NAME_MAX 255
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

#ifndef S_IFLNK
#define S_IFLNK 0120000
#endif
#ifndef S_IFSOCK
#define S_IFSOCK 0140000
#endif

/* newlib may lack these POSIX helpers; stubs in myos_stubs.c */


#ifndef DT_UNKNOWN
#define DT_UNKNOWN 0
#endif
#ifndef DT_FIFO
#define DT_FIFO 1
#endif
#ifndef DT_CHR
#define DT_CHR 2
#endif
#ifndef DT_DIR
#define DT_DIR 4
#endif
#ifndef DT_BLK
#define DT_BLK 6
#endif
#ifndef DT_REG
#define DT_REG 8
#endif
#ifndef DT_LNK
#define DT_LNK 10
#endif
#ifndef DT_SOCK
#define DT_SOCK 12
#endif
#ifndef DT_WHT
#define DT_WHT 14
#endif


#ifndef SA_RESTART
#define SA_RESTART 0x10000000
#endif
#ifndef SA_NOCLDSTOP
#define SA_NOCLDSTOP 1
#endif
#ifndef SA_NOCLDWAIT
#define SA_NOCLDWAIT 2
#endif
#ifndef SA_NODEFER
#define SA_NODEFER 0x40000000
#endif
#ifndef SA_RESETHAND
#define SA_RESETHAND 0x80000000
#endif
#ifndef SA_SIGINFO
#define SA_SIGINFO 4
#endif


#ifndef AT_FDCWD
#define AT_FDCWD (-100)
#endif


ssize_t getdelim(char **lineptr, size_t *n, int delim, FILE *stream);
ssize_t getline(char **lineptr, size_t *n, FILE *stream);
struct utsname;
int uname(struct utsname *buf);

#endif /* _MYOS_GIT_COMPAT_H_ */
