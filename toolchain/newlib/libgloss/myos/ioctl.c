/* myos libgloss: tty ioctl via SYS_IOCTL (getty/login).
 * TIOCSCTTY is implemented in the kernel (sets the process ctty). */
#include <errno.h>
#include <stdarg.h>
#include <sys/ioctl.h>
#include <unistd.h>

#include "myos_syscalls.h"

int ioctl(int fd, unsigned long request, ...) {
    va_list ap;
    void *arg;

    if (fd < 0) {
        errno = EBADF;
        return -1;
    }

    va_start(ap, request);
    arg = va_arg(ap, void *);
    va_end(ap);

    switch (request) {
    case TIOCSCTTY:
    case TCFLSH:
    case TIOCGWINSZ:
    case TCGETS:
    case TCSETS:
        break;
    default:
        errno = ENOTTY;
        return -1;
    }

    if ((unsigned long)myos_syscall3(MYOS_SYS_IOCTL, fd, (long)request, (long)arg) == MYOS_SYSERR) {
        errno = ENOTTY;
        return -1;
    }
    return 0;
}
