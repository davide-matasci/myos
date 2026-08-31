#ifndef _MYOS_SYS_PARAM_H_
#define _MYOS_SYS_PARAM_H_

#include <limits.h>
#include <sys/types.h>

#ifndef MAXPATHLEN
#define MAXPATHLEN 1024
#endif
#ifndef MAXLOGNAME
#define MAXLOGNAME 32
#endif
#ifndef ALIGNBYTES
#define ALIGNBYTES (sizeof(long) - 1)
#endif
#ifndef ALIGN
#define ALIGN(p) (((unsigned long)(p) + ALIGNBYTES) & ~ALIGNBYTES)
#endif

#endif /* _MYOS_SYS_PARAM_H_ */
