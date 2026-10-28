/* myos: WebKit (built as for Linux, packages/webkitgtk/build.sh) names
 * its threads and memory regions and tunes the kernel's timers with
 * prctl. myos has none: every call fails with EINVAL, which WebKit
 * ignores (the names are for debuggers, the rest are hints). */
#ifndef MYOS_COMPAT_SYS_PRCTL_H
#define MYOS_COMPAT_SYS_PRCTL_H

#include <errno.h>

/* Linux's values (linux/prctl.h). */
#define PR_SET_NAME 15
#define PR_SET_TIMERSLACK 29
#define PR_SET_THP_DISABLE 41
#define PR_GET_THP_DISABLE 42
#define PR_SET_VMA 0x53564d41
#define PR_SET_VMA_ANON_NAME 0

static inline int prctl(int option, ...)
{
    (void)option;
    errno = EINVAL;
    return -1;
}

#endif
