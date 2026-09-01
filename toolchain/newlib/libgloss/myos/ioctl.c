/* myos libgloss: tty ioctl nops getty/login need (no real session yet). */
#include <errno.h>
#include <stdarg.h>
#include <sys/ioctl.h>
#include <unistd.h>

int ioctl(int fd, unsigned long request, ...) {
    va_list ap;

    if (fd < 0) {
        errno = EBADF;
        return -1;
    }

    va_start(ap, request);
    switch (request) {
    case TIOCSCTTY:
        (void)va_arg(ap, void *);
        va_end(ap);
        return 0;
    case TCFLSH:
        (void)va_arg(ap, void *);
        va_end(ap);
        return 0;
    case TIOCGWINSZ: {
        struct winsize *ws = va_arg(ap, struct winsize *);
        va_end(ap);
        if (ws == NULL) {
            errno = EFAULT;
            return -1;
        }
        ws->ws_row = 24;
        ws->ws_col = 80;
        ws->ws_xpixel = 0;
        ws->ws_ypixel = 0;
        return 0;
    }
    case TCGETS:
    case TCSETS:
        (void)va_arg(ap, void *);
        va_end(ap);
        return 0;
    default:
        va_end(ap);
        errno = ENOTTY;
        return -1;
    }
}
