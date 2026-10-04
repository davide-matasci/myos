/* Included by every compilation unit of the font stack (myos_write_cross_cc
 * in build.sh), after x11-libs' own compat header: the libc bits newlib keeps
 * elsewhere. */
#ifndef MYOS_X11_XFT_COMPAT_H
#define MYOS_X11_XFT_COMPAT_H

#include <sys/types.h>
#include <sys/stat.h>
/* lstat (fontconfig's directory scan). */
#include <sys/myos_extra.h>

#endif
