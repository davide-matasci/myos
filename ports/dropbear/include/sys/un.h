/* Minimal <sys/un.h> for myos (newlib lacks it; dropbear includes it
 * unconditionally even with AF_UNIX channels disabled). */
#ifndef DROPBEAR_MYOS_SYS_UN_H
#define DROPBEAR_MYOS_SYS_UN_H

#include <sys/socket.h>

#ifndef AF_UNIX
#define AF_UNIX 1
#endif
#ifndef AF_LOCAL
#define AF_LOCAL AF_UNIX
#endif
#ifndef PF_UNIX
#define PF_UNIX AF_UNIX
#endif
#ifndef PF_LOCAL
#define PF_LOCAL AF_UNIX
#endif

struct sockaddr_un {
	sa_family_t sun_family;
	char sun_path[108];
};

#endif /* DROPBEAR_MYOS_SYS_UN_H */
