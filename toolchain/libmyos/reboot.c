/* reboot(2) over the native power(action). The RB_ names are the ones both
 * libcs' <sys/reboot.h> carry (newlib's: libgloss/myos/sys/reboot.h). */
#include <errno.h>
#include <sys/reboot.h>

#include "myos_syscalls.h"
#include "libc.h"

int __myos_reboot(int howto) {
    long action;
    switch (howto) {
    case RB_AUTOBOOT:
        action = MYOS_POWER_REBOOT;
        break;
    case RB_HALT_SYSTEM:
        action = MYOS_POWER_HALT;
        break;
    case RB_POWER_OFF:
        action = MYOS_POWER_OFF;
        break;
    default:
        errno = EINVAL;
        return -1;
    }
    myos_syscall1(MYOS_SYS_POWER, action);
    errno = EPERM;
    return -1;
}

MYOS_ALIAS(reboot)
