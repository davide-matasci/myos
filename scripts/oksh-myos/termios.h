/* Compile-only dummy so tty.h/jobs.c/edit.c parse. oksh never calls
 * tcsetattr at runtime (main.myos.patch skips x_init). Not installed
 * into the newlib sysroot. */
#ifndef _TERMIOS_H_
#define _TERMIOS_H_

#include <sys/types.h>

#define NCCS 20
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

#define INLCR 0000100
#define ICRNL 0000400
#define ISIG 0000001
#define ICANON 0000002
#define ECHO 0000010

#ifndef _POSIX_VDISABLE
#define _POSIX_VDISABLE 0
#endif

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
