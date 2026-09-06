/* myos libgloss: POSIX termios types/APIs for oksh/vim/etc.
 * Kernel stdin stays cooked; tcgetattr fails / tcsetattr succeeds as no-ops
 * (see termios.c). Installed into the newlib sysroot as <termios.h>. */
#ifndef _TERMIOS_H_
#define _TERMIOS_H_

#include <sys/types.h>

#define NCCS 32
#define TCSANOW 0
#define TCSADRAIN 1
#define TCSAFLUSH 2

#define VINTR 0
#define VQUIT 1
#define VERASE 2
#define VKILL 3
#define VEOF 4
#define VTIME 5
#define VMIN 6
#define VSUSP 10

#define IGNBRK 0000001
#define BRKINT 0000002
#define IGNPAR 0000004
#define PARMRK 0000010
#define INPCK 0000020
#define ISTRIP 0000040
#define INLCR 0000100
#define IGNCR 0000200
#define ICRNL 0000400
#define IXON 0002000
#define IXOFF 0010000

#define OPOST 0000001
#define ONLCR 0000004

#define ISIG 0000001
#define ICANON 0000002
#define ECHO 0000010
#define ECHOE 0000020
#define ECHOK 0000040
#define ECHONL 0000100
#define NOFLSH 0000200
#define TOSTOP 0000400
#define IEXTEN 0001000

#define CSIZE 0000060
#define CS8 0000060
#define CREAD 0000200
#define CLOCAL 0004000

/* Queue selectors / flow — also in sys/ioctl.h; keep matching values. */
#ifndef TCIFLUSH
#define TCIFLUSH 0
#endif
#ifndef TCOFLUSH
#define TCOFLUSH 1
#endif
#ifndef TCIOFLUSH
#define TCIOFLUSH 2
#endif
#define TCOOFF 0
#define TCOON 1
#define TCIOFF 2
#define TCION 3

#ifndef _POSIX_VDISABLE
#define _POSIX_VDISABLE '\0'
#endif


/* Baud-rate constants (for ncurses/tinfo; unused by kernel stubs). */
#define B0 0
#define B50 1
#define B75 2
#define B110 3
#define B134 4
#define B150 5
#define B200 6
#define B300 7
#define B600 8
#define B1200 9
#define B1800 10
#define B2400 11
#define B4800 12
#define B9600 13
#define B19200 14
#define B38400 15
#define B57600 16
#define B115200 17
#define B230400 18

typedef unsigned char cc_t;
typedef unsigned int tcflag_t;
typedef unsigned int speed_t;

struct termios {
    tcflag_t c_iflag;
    tcflag_t c_oflag;
    tcflag_t c_cflag;
    tcflag_t c_lflag;
    cc_t c_cc[NCCS];
    speed_t c_ispeed;
    speed_t c_ospeed;
};

int tcgetattr(int fd, struct termios *termios_p);
int tcsetattr(int fd, int optional_actions, const struct termios *termios_p);
int tcsendbreak(int fd, int duration);
int tcdrain(int fd);
int tcflush(int fd, int queue_selector);
int tcflow(int fd, int action);
pid_t tcgetpgrp(int fd);
int tcsetpgrp(int fd, pid_t pgrp);
speed_t cfgetispeed(const struct termios *termios_p);
speed_t cfgetospeed(const struct termios *termios_p);
int cfsetispeed(struct termios *termios_p, speed_t speed);
int cfsetospeed(struct termios *termios_p, speed_t speed);

#endif /* _TERMIOS_H_ */
