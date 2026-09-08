/*	$OpenBSD: tty.c,v 1.19 2021/10/24 21:24:21 deraadt Exp $	*/

/*
 * myos: full replacement tty.c. There is no /dev/tty or F_DUPFD; the
 * console (stdin, fd 0) is the controlling tty and is never closed.
 * termios (tcgetattr/tcsetattr) comes from libgloss via TCGETS/TCSETS.
 */

#include <errno.h>
#include <fcntl.h>
#include <string.h>
#include <unistd.h>

#include "sh.h"
#include "tty.h"

int		tty_fd = -1;	/* console handle (stdin, fd 0) */
int		tty_devtty;	/* 0: tty_fd is the console, not /dev/tty */
struct termios	tty_state;	/* saved (cooked) tty state for x_mode() */

void
tty_close(void)
{
	/* tty_fd is the shell's console; never close it or the foreground
	 * child's stdin would be lost. tty_fd is only a handle.
	 */
	tty_fd = -1;
}

/* Initialize tty_fd.  Used for saving/resetting tty modes upon
 * foreground job completion and for setting up tty process group.
 */
void
tty_init(int init_ttystate)
{
	(void)init_ttystate;
	tty_close();
	tty_devtty = 0;
	tty_fd = 0;	/* console = stdin */
	if (init_ttystate)
		tcgetattr(tty_fd, &tty_state);	/* save cooked state for x_mode() */
}