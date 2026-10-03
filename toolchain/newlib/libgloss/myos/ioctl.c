/* myos libgloss: tty ioctl via SYS_IOCTL (getty/login), and the /dev/fb0
 * and console-mode ones (linux/fb.h, linux/kd.h).
 * TIOCSCTTY is implemented in the kernel (sets the process ctty). */
#include <errno.h>
#include <stdarg.h>
#include <sys/ioctl.h>
#include <unistd.h>
#include <linux/fb.h>
#include <linux/kd.h>

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
    case TIOCGPTN:
    case TIOCSPTLCK:
    case TCGETS:
    case TCSETS:
    case FBIOGET_VSCREENINFO:
    case FBIOPUT_VSCREENINFO:
    case FBIOGET_FSCREENINFO:
    case FBIOPAN_DISPLAY:
    case FBIOBLANK:
    case KDSETMODE:
    case KDGETMODE:
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
