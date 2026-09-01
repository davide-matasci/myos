/* Compile-only for oksh (`#include <paths.h>`). Not installed into the
 * newlib sysroot. */
#ifndef _PATHS_H_
#define _PATHS_H_

#ifndef _PATH_BSHELL
#define _PATH_BSHELL "/sh"
#endif
#ifndef _PATH_DEFPATH
#define _PATH_DEFPATH "/s:/c:/"
#endif
#ifndef _PATH_STDPATH
#define _PATH_STDPATH "/s:/c:/"
#endif
#ifndef _CS_PATH
#define _CS_PATH 1
#endif

#endif /* _PATHS_H_ */
