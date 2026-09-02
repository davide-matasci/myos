/* myos TinyCC config: native -run, no host configure. */
#ifndef TCC_VERSION
#define TCC_VERSION "0.9.28rc"
#endif
#define CONFIG_TCCDIR "/t"
#define CONFIG_TCC_STATIC 1
#define CONFIG_TCC_PIE 1
#define CONFIG_TCC_SEMLOCK 0
#define CONFIG_TCC_BCHECK 0
#define CONFIG_TCC_BACKTRACE 0
#define CONFIG_TCC_PREDEFS 1
#define CONFIG_TCC_SYSINCLUDEPATHS "/lib/newlib/include"
#define CONFIG_TCC_LIBPATHS "/lib/newlib/lib"
#define CONFIG_TCC_CRTPREFIX "/lib/newlib/lib"
