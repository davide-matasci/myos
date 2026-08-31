/* Compile-only dummy so tty.h/jobs.c parse. oksh never calls tcsetattr
 * (no emacs/vi raw tty). Not installed into the newlib sysroot. */
#ifndef _TERMIOS_H_
#define _TERMIOS_H_

#include <sys/types.h>

#define NCCS 20
#define TCSANOW 0
#define TCSADRAIN 1
#define TCSAFLUSH 2

typedef unsigned char cc_t;
typedef unsigned int tcflag_t;
typedef unsigned int speed_t;

struct termios {
    tcflag_t c_iflag;
    tcflag_t c_oflag;
    tcflag_t c_cflag;
    tcflag_t c_lflag;
    cc_t c_cc[NCCS];
};

#endif /* _TERMIOS_H_ */
