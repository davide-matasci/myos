#ifndef _MYOS_LINUX_KD_H_
#define _MYOS_LINUX_KD_H_

/* Console text/graphics mode (docs/fb.md): KD_GRAPHICS stops the console
 * painting the screen while a program draws on /dev/fb0; KD_TEXT, or the
 * program's exit, gives it back. On a console tty or on /dev/fb0. */

#define KDSETMODE   0x4B3A
#define KDGETMODE   0x4B3B
#define KD_TEXT     0
#define KD_GRAPHICS 1

#endif /* _MYOS_LINUX_KD_H_ */
