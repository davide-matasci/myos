/* Force-included for lynx on myos (missing POSIX bits / path tweaks). */
#ifndef MYOS_LYNX_COMPAT_H
#define MYOS_LYNX_COMPAT_H

#include <sys/types.h>
#include <stdint.h>
#include <sys/stat.h>
#include <sys/myos_extra.h>

#ifndef __myos__
#define __myos__ 1
#endif

/* newlib may lack these; provide conservative defaults */
#ifndef MAXPATHLEN
#define MAXPATHLEN 1024
#endif

#ifndef NI_MAXHOST
#define NI_MAXHOST 1025
#endif

#endif /* MYOS_LYNX_COMPAT_H */
