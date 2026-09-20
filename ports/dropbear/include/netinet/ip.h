/* Minimal <netinet/ip.h> for myos (newlib lacks it; dropbear includes it
 * unconditionally but only uses the struct for X11/TCP forwarding, which is
 * disabled). Header layout matches the BSD classic form. */
#ifndef DROPBEAR_MYOS_NETINET_IP_H
#define DROPBEAR_MYOS_NETINET_IP_H

#include <netinet/in_systm.h>
#include <netinet/in.h>

struct ip {
	unsigned char ip_hl:4, ip_v:4;
	unsigned char ip_tos;
	unsigned short ip_len;
	unsigned short ip_id;
	unsigned short ip_off;
	unsigned char ip_ttl;
	unsigned char ip_p;
	unsigned short ip_sum;
	struct in_addr ip_src, ip_dst;
};

#define IPVERSION 4
#define IPOPT_EOL 0
#define IPOPT_NOP 1

#endif /* DROPBEAR_MYOS_NETINET_IP_H */
