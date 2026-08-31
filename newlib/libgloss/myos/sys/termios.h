#ifndef _MYOS_SYS_TERMIOS_H_
#define _MYOS_SYS_TERMIOS_H_

#include <sys/types.h>

#define NCCS 20

#define TCSANOW 0
#define TCSADRAIN 1
#define TCSAFLUSH 2

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
#define ONLCR 0000002

#define CSIZE 0000060
#define CS8 0000060
#define CREAD 0000200
#define CLOCAL 0004000

#define ISIG 0000001
#define ICANON 0000002
#define ECHO 0000010
#define ECHOE 0000020
#define ECHOK 0000040
#define ECHONL 0000100
#define IEXTEN 0000400
#define TOSTOP 0001000
#define NOFLSH 0000200

#define VINTR 0
#define VQUIT 1
#define VERASE 2
#define VKILL 3
#define VEOF 4
#define VTIME 5
#define VMIN 6
#define VSWTC 7
#define VSTART 8
#define VSTOP 9
#define VSUSP 10
#define VEOL 11
#define VREPRINT 12
#define VDISCARD 13
#define VWERASE 14
#define VLNEXT 15
#define VEOL2 16

#define _POSIX_VDISABLE '\0'

#define B0 0
#define B9600 15
#define B38400 16

typedef unsigned char cc_t;
typedef unsigned int tcflag_t;
typedef unsigned int speed_t;

struct termios {
    tcflag_t c_iflag;
    tcflag_t c_oflag;
    tcflag_t c_cflag;
    tcflag_t c_lflag;
    cc_t c_line;
    cc_t c_cc[NCCS];
    speed_t c_ispeed;
    speed_t c_ospeed;
};

int tcgetattr(int fd, struct termios *t);
int tcsetattr(int fd, int optional_actions, const struct termios *t);
pid_t tcgetpgrp(int fd);
int tcsetpgrp(int fd, pid_t pgrp);

#endif /* _MYOS_SYS_TERMIOS_H_ */
