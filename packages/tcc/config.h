/* myos TinyCC config: native -run, no host configure. */
#ifndef TCC_VERSION
#define TCC_VERSION "0.9.28rc"
#endif
#define CONFIG_TCCDIR "/bin/tcc"
#define CONFIG_TCC_STATIC 1
#define CONFIG_TCC_PIE 1
#define CONFIG_TCC_SEMLOCK 0
#define CONFIG_TCC_BCHECK 0
#define CONFIG_TCC_BACKTRACE 0
#define CONFIG_TCC_PREDEFS 1
/* /lib/x11: the X headers and libraries of the x11-dev package, there
   when it is installed (packages/x11-dev). */
#define CONFIG_TCC_SYSINCLUDEPATHS "/lib/newlib/include:/lib/x11/include"
#define CONFIG_TCC_LIBPATHS "/lib/newlib/lib:/lib/x11/lib"
#define CONFIG_TCC_CRTPREFIX "/lib/newlib/lib"
