#ifndef _MYOS_SYS_IOCTL_H_
#define _MYOS_SYS_IOCTL_H_

#include <sys/types.h>

/* Linux tty ioctls used by ubase getty/login and termios (kernel-backed). */
#define TCGETS 0x5401
#define TCSETS 0x5402
#define TCFLSH 0x540B
#define TIOCSCTTY 0x540E
#define TIOCGWINSZ 0x5413
#define TCIFLUSH 0
#define TCOFLUSH 1
#define TCIOFLUSH 2

struct winsize {
    unsigned short ws_row;
    unsigned short ws_col;
    unsigned short ws_xpixel;
    unsigned short ws_ypixel;
};

int ioctl(int fd, unsigned long request, ...);

#endif /* _MYOS_SYS_IOCTL_H_ */
