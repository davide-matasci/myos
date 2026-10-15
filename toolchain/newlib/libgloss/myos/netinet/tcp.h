#ifndef _MYOS_NETINET_TCP_H_
#define _MYOS_NETINET_TCP_H_

/* The TCP socket options: setsockopt accepts and ignores them (netd's
 * smoltcp sockets keep their own settings, docs/sockets-curl.md). */
#define TCP_NODELAY 1
#define TCP_MAXSEG 2
#define TCP_CORK 3
#define TCP_KEEPIDLE 4
#define TCP_KEEPINTVL 5
#define TCP_KEEPCNT 6

#endif
