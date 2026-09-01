/* This file is the checked-in equivalent of oksh `configure --no-thanks
 * --enable-small --disable-curses` (do not run configure on the myos triple:
 * it tries to execute conftest). HAVE_* flags match newlib 4.4.0.
 *
 * HAVE_CONFSTR is off: link oksh's bundled confstr.c (oksh compat, not a
 * newlib replacement). PATH is still forced in main.myos.patch.
 */

#ifndef __dead
#define __dead __attribute__((__noreturn__))
#endif

/* #define __attribute__(x) */

#define HAVE_ASPRINTF
/* #define HAVE_CONFSTR */
#define NO_CURSES
/* #define HAVE_ISSETUGID */
/* #define HAVE_GETAUXVAL */
/* #define HAVE_PLEDGE */
#define HAVE_REALLOCARRAY
/* #define HAVE_SETRESGID */
/* #define HAVE_SETRESUID */
/* #define HAVE_SIG_T */
/* #define HAVE_SRAND_DETERMINISTIC */
#define HAVE_ST_MTIM
/* #define HAVE_ST_MTIMESPEC */
/* #define HAVE_STRAVIS */
#define HAVE_STRLCAT
#define HAVE_STRLCPY
/* #define HAVE_STRTONUM */
/* #define HAVE_STRUNVIS */
/* #define HAVE_SIGLIST */
/* #define HAVE_SIGNAME */
/* #define HAVE_TIMERADD */
/* #define HAVE_TIMERCLEAR */
/* #define HAVE_TIMERSUB */
