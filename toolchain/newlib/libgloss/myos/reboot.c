/* myos libgloss: reboot(2) over the native power(action). */
#include <errno.h>
#include <sys/reboot.h>

#include "myos_syscalls.h"

int reboot(int howto) {
    long action;
    switch (howto) {
    case RB_AUTOBOOT:
        action = MYOS_POWER_REBOOT;
        break;
    case RB_HALT:
        action = MYOS_POWER_HALT;
        break;
    case RB_POWEROFF:
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
