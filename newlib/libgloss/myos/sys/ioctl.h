#ifndef _MYOS_SYS_IOCTL_H_
#define _MYOS_SYS_IOCTL_H_

#include <sys/types.h>

#define TIOCGWINSZ 0x5413

struct winsize {
    unsigned short ws_row;
    unsigned short ws_col;
    unsigned short ws_xpixel;
    unsigned short ws_ypixel;
};

static inline int ioctl(int fd, unsigned long request, ...) {
    (void)fd;
    (void)request;
    return -1;
}

#endif /* _MYOS_SYS_IOCTL_H_ */
