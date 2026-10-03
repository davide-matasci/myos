#ifndef _MYOS_SYS_UN_H_
#define _MYOS_SYS_UN_H_

/* AF_UNIX addresses (docs/sockets-unix.md). sun_path is a name in
 * /net/unix, not a file: bind() creates nothing in the filesystem. */

#include <sys/socket.h>

struct sockaddr_un {
    sa_family_t sun_family;
    char sun_path[108];
};

#endif /* _MYOS_SYS_UN_H_ */
